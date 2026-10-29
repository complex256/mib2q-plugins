// tsstatus serves tailscaled's state as a few key=value lines on a loopback
// port, for the mib2q-plugins tailscale-status plugin.
//
// It polls tailscaled's LocalAPI over the unix socket every 10 s and caches
// the reduced result; a client connecting to 127.0.0.1:19850 is sent the cache
// and the connection is closed. No tailscale import: the LocalAPI reply is
// parsed into a local struct holding only the fields used here.
//
// The unit's clock sits at 1970 (tailscaled alone sees corrected time through
// libtsclock.so), so every figure is an age in seconds, never a timestamp.
// The handshake age needs corrected "now": it is time.Now() plus the offset
// that tailscale.sh keeps in /tmp/ts_clock_offset.
package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"os"
	"os/signal"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"
)

const (
	defaultAddr   = "127.0.0.1:19850"
	defaultSock   = "/tmp/tailscaled.sock"
	defaultOffset = "/tmp/ts_clock_offset"
	pollEvery     = 10 * time.Second
	pollTimeout   = 5 * time.Second
	writeTimeout  = 2 * time.Second
	drainTimeout  = 150 * time.Millisecond
	acceptRetry   = 100 * time.Millisecond
)

// localStatus is the part of ipnstate.Status this program reads.
type localStatus struct {
	BackendState   string
	TailscaleIPs   []string
	CurrentTailnet *struct{ Name string }
	Peer           map[string]struct {
		Online        bool
		LastHandshake time.Time
	}
}

// status is the reduced view that gets served.
type status struct {
	state           string
	ip, tailnet     string
	peers, online   int
	handshakeAgeSec int // -1 when no peer has ever handshaken
}

func downStatus() status { return status{state: "down", handshakeAgeSec: -1} }

// reduceStatus turns a LocalAPI /status body into a status. now is corrected
// wall time. A handshake that is zero or in the future (offset not yet known)
// does not count, so the age is -1 rather than a wrong number.
func reduceStatus(body []byte, now time.Time) (status, error) {
	var in localStatus
	if err := json.Unmarshal(body, &in); err != nil {
		return status{}, err
	}
	s := status{state: in.BackendState, handshakeAgeSec: -1, peers: len(in.Peer)}
	if s.state == "" {
		s.state = "down"
	}
	if len(in.TailscaleIPs) > 0 {
		s.ip = in.TailscaleIPs[0]
	}
	if in.CurrentTailnet != nil {
		s.tailnet = in.CurrentTailnet.Name
	}
	for _, p := range in.Peer {
		if p.Online {
			s.online++
		}
		if p.LastHandshake.IsZero() || p.LastHandshake.Year() < 2000 {
			continue
		}
		age := int(now.Sub(p.LastHandshake) / time.Second)
		if age < 0 {
			continue
		}
		if s.handshakeAgeSec < 0 || age < s.handshakeAgeSec {
			s.handshakeAgeSec = age
		}
	}
	return s, nil
}

// fields renders every line except the version header and age_s.
func (s status) fields() string {
	return fmt.Sprintf("state=%s\nip=%s\ntailnet=%s\npeers=%d\npeers_online=%d\nlast_handshake_age_s=%d\n",
		s.state, s.ip, s.tailnet, s.peers, s.online, s.handshakeAgeSec)
}

// cache holds the latest poll result.
type cache struct {
	mu sync.Mutex
	s  status
	at time.Time
}

// newCache starts as "down" with the age counted from now, so a client that
// connects before the first poll gets an honest answer.
func newCache() *cache { return &cache{s: downStatus(), at: time.Now()} }

func (c *cache) set(s status, at time.Time) {
	c.mu.Lock()
	c.s, c.at = s, at
	c.mu.Unlock()
}

func (c *cache) render(now time.Time) string {
	c.mu.Lock()
	s, at := c.s, c.at
	c.mu.Unlock()
	age := int(now.Sub(at) / time.Second)
	if age < 0 {
		age = 0
	}
	return "v=1\ndaemon=tailscale\n" + s.fields() + "age_s=" + strconv.Itoa(age) + "\n"
}

// correctedNow is time.Now() plus the clock offset in offsetFile (whole
// seconds, as tailscale.sh writes it); no readable file means no correction.
func correctedNow(offsetFile string) time.Time {
	now := time.Now()
	if offsetFile == "" {
		return now
	}
	b, err := os.ReadFile(offsetFile)
	if err != nil {
		return now
	}
	n, err := strconv.ParseInt(strings.TrimSpace(string(b)), 10, 64)
	if err != nil {
		return now
	}
	return now.Add(time.Duration(n) * time.Second)
}

// fetchStatus asks tailscaled's LocalAPI over its unix socket.
func fetchStatus(sock string, timeout time.Duration) ([]byte, error) {
	cl := &http.Client{
		Timeout: timeout,
		Transport: &http.Transport{
			DisableKeepAlives: true,
			DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
				return (&net.Dialer{}).DialContext(ctx, "unix", sock)
			},
		},
	}
	resp, err := cl.Get("http://local-tailscaled.sock/localapi/v0/status")
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("localapi status: %s", resp.Status)
	}
	return io.ReadAll(io.LimitReader(resp.Body, 8<<20))
}

// pollOnce refreshes the cache; any failure (socket missing, timeout, bad
// reply) is state=down.
func pollOnce(c *cache, sock, offsetFile string, timeout time.Duration) {
	s := downStatus()
	if body, err := fetchStatus(sock, timeout); err == nil {
		if r, err := reduceStatus(body, correctedNow(offsetFile)); err == nil {
			s = r
		}
	}
	c.set(s, time.Now())
}

func pollLoop(ctx context.Context, c *cache, sock, offsetFile string) {
	t := time.NewTicker(pollEvery)
	defer t.Stop()
	for {
		pollOnce(c, sock, offsetFile, pollTimeout)
		select {
		case <-ctx.Done():
			return
		case <-t.C:
		}
	}
}

// serve writes the cached reply to each connection and closes it.
func serve(ctx context.Context, l net.Listener, c *cache) {
	for {
		conn, err := l.Accept()
		if err != nil {
			if ctx.Err() != nil || errors.Is(err, net.ErrClosed) {
				return
			}
			// Transient (EMFILE, ECONNABORTED...): the status endpoint
			// should outlive a bad moment, not exit.
			log.Printf("tsstatus: accept: %v", err)
			time.Sleep(acceptRetry)
			continue
		}
		go reply(conn, c)
	}
}

// reply sends the cache, half-closes, drains whatever the client sent for a
// moment, then closes. Closing with unread data pending makes the kernel send
// an RST that can discard the reply (`echo | nc`).
func reply(conn net.Conn, c *cache) {
	defer conn.Close()
	conn.SetWriteDeadline(time.Now().Add(writeTimeout))
	if _, err := conn.Write([]byte(c.render(time.Now()))); err != nil {
		return
	}
	if tc, ok := conn.(*net.TCPConn); ok {
		tc.CloseWrite()
	}
	conn.SetReadDeadline(time.Now().Add(drainTimeout))
	io.Copy(io.Discard, io.LimitReader(conn, 64<<10))
}

// checkLoopback rejects any listen address that is not a loopback host with a
// port: the endpoint is for on-unit clients only.
func checkLoopback(addr string) error {
	host, _, err := net.SplitHostPort(addr)
	if err != nil {
		return err
	}
	if host == "localhost" {
		return nil
	}
	if ip := net.ParseIP(host); ip != nil && ip.IsLoopback() {
		return nil
	}
	return fmt.Errorf("%q is not a loopback address", addr)
}

func main() {
	addr := flag.String("addr", defaultAddr, "listen address (loopback)")
	sock := flag.String("socket", defaultSock, "tailscaled unix socket")
	offset := flag.String("clock-offset-file", defaultOffset, "file holding the clock offset in seconds")
	flag.Parse()

	if err := checkLoopback(*addr); err != nil {
		log.Fatalf("tsstatus: --addr: %v", err)
	}
	l, err := net.Listen("tcp", *addr)
	if err != nil {
		log.Fatalf("tsstatus: %v", err)
	}
	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGTERM, syscall.SIGINT)
	defer stop()
	go func() { <-ctx.Done(); l.Close() }()

	c := newCache()
	go pollLoop(ctx, c, *sock, *offset)
	serve(ctx, l, c)
}
