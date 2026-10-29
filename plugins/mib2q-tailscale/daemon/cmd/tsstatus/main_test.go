package main

import (
	"context"
	"io"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"
)

const fixture = `{
  "Version": "1.102.4",
  "BackendState": "Running",
  "TailscaleIPs": ["100.64.0.7", "fd7a:115c:a1e0::7"],
  "CurrentTailnet": {"Name": "example.ts.net", "MagicDNSSuffix": "x"},
  "Peer": {
    "k1": {"Online": true,  "LastHandshake": "2026-10-19T11:59:30Z"},
    "k2": {"Online": true,  "LastHandshake": "2026-10-19T11:59:50Z"},
    "k3": {"Online": false, "LastHandshake": "0001-01-01T00:00:00Z"}
  }
}`

var noon = time.Date(2026, 10, 19, 12, 0, 0, 0, time.UTC)

func TestReduceStatus(t *testing.T) {
	s, err := reduceStatus([]byte(fixture), noon)
	if err != nil {
		t.Fatal(err)
	}
	want := "state=Running\nip=100.64.0.7\ntailnet=example.ts.net\npeers=3\npeers_online=2\nlast_handshake_age_s=10\n"
	if got := s.fields(); got != want {
		t.Fatalf("fields:\n%q\nwant\n%q", got, want)
	}
}

func TestReduceStatusEmptyAndNoHandshake(t *testing.T) {
	s, err := reduceStatus([]byte(`{"BackendState":"NeedsLogin"}`), noon)
	if err != nil {
		t.Fatal(err)
	}
	want := "state=NeedsLogin\nip=\ntailnet=\npeers=0\npeers_online=0\nlast_handshake_age_s=-1\n"
	if got := s.fields(); got != want {
		t.Fatalf("fields:\n%q\nwant\n%q", got, want)
	}
	// A handshake in the future (clock offset not yet known) is not an age.
	s, _ = reduceStatus([]byte(`{"BackendState":"Running","Peer":{"k":{"LastHandshake":"2026-10-19T12:00:05Z"}}}`), noon)
	if !strings.Contains(s.fields(), "last_handshake_age_s=-1\n") {
		t.Fatalf("future handshake: %q", s.fields())
	}
	if _, err := reduceStatus([]byte(`not json`), noon); err == nil {
		t.Fatal("want error for bad json")
	}
}

func TestDownWhenSocketMissing(t *testing.T) {
	c := newCache()
	pollOnce(c, filepath.Join(t.TempDir(), "nope.sock"), "", time.Second)
	got := c.render(time.Now())
	if !strings.HasPrefix(got, "v=1\ndaemon=tailscale\nstate=down\nip=\ntailnet=\npeers=0\npeers_online=0\nlast_handshake_age_s=-1\nage_s=") {
		t.Fatalf("got %q", got)
	}
}

func TestDownWhenPollTimesOut(t *testing.T) {
	sock := filepath.Join(t.TempDir(), "s.sock")
	l, err := net.Listen("unix", sock)
	if err != nil {
		t.Skip("unix sockets unavailable:", err)
	}
	defer l.Close()
	go func() { // accept and never answer
		for {
			c, err := l.Accept()
			if err != nil {
				return
			}
			defer c.Close()
		}
	}()
	c := newCache()
	c.set(parsedOK(t), noon)
	pollOnce(c, sock, "", 200*time.Millisecond)
	if !strings.Contains(c.render(time.Now()), "state=down\n") {
		t.Fatal("timeout did not yield state=down")
	}
}

func parsedOK(t *testing.T) status {
	s, err := reduceStatus([]byte(fixture), noon)
	if err != nil {
		t.Fatal(err)
	}
	return s
}

func TestServesCachedAndCloses(t *testing.T) {
	c := newCache()
	c.set(parsedOK(t), time.Now().Add(-7*time.Second))
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan struct{})
	go func() { serve(ctx, l, c); close(done) }()

	conn, err := net.Dial("tcp", l.Addr().String())
	if err != nil {
		t.Fatal(err)
	}
	conn.SetDeadline(time.Now().Add(2 * time.Second))
	b, err := io.ReadAll(conn) // returns only if the server closes
	if err != nil {
		t.Fatal(err)
	}
	conn.Close()
	got := string(b)
	for _, w := range []string{"v=1\ndaemon=tailscale\nstate=Running\n", "ip=100.64.0.7\n", "peers=3\n", "age_s=7\n"} {
		if !strings.Contains(got, w) {
			t.Errorf("missing %q in %q", w, got)
		}
	}
	cancel()
	l.Close()
	<-done
}

// A client that sends before reading (`echo | nc`) must still get the reply:
// closing with unread data pending would turn into an RST that discards it.
func TestServesClientThatWritesFirst(t *testing.T) {
	c := newCache()
	c.set(parsedOK(t), time.Now())
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan struct{})
	go func() { serve(ctx, l, c); close(done) }()
	defer func() { cancel(); l.Close(); <-done }()

	for i := 0; i < 20; i++ {
		conn, err := net.Dial("tcp", l.Addr().String())
		if err != nil {
			t.Fatal(err)
		}
		conn.SetDeadline(time.Now().Add(2 * time.Second))
		if _, err := conn.Write([]byte("hello\n")); err != nil {
			t.Fatal(err)
		}
		b, err := io.ReadAll(conn)
		conn.Close()
		if err != nil {
			t.Fatalf("iteration %d: %v", i, err)
		}
		if !strings.HasPrefix(string(b), "v=1\ndaemon=tailscale\nstate=Running\n") {
			t.Fatalf("iteration %d: got %q", i, b)
		}
	}
}

func TestCheckLoopback(t *testing.T) {
	for _, ok := range []string{"127.0.0.1:19850", "[::1]:19850", "localhost:19850", "127.0.0.1:0"} {
		if err := checkLoopback(ok); err != nil {
			t.Errorf("%s rejected: %v", ok, err)
		}
	}
	for _, bad := range []string{":19850", "0.0.0.0:19850", "192.168.1.5:19850", "[::]:19850", "garbage", "example.com:80"} {
		if err := checkLoopback(bad); err == nil {
			t.Errorf("%s accepted", bad)
		}
	}
}

func TestCorrectedNowOffsetFile(t *testing.T) {
	dir := t.TempDir()
	near := func(got, want time.Time) bool { d := got.Sub(want); return d > -2*time.Second && d < 2*time.Second }
	if !near(correctedNow(""), time.Now()) {
		t.Error("no file: want plain now")
	}
	if !near(correctedNow(filepath.Join(dir, "missing")), time.Now()) {
		t.Error("missing file: want plain now")
	}
	bad := filepath.Join(dir, "bad")
	os.WriteFile(bad, []byte("abc\n"), 0o644)
	if !near(correctedNow(bad), time.Now()) {
		t.Error("garbage file: want plain now")
	}
	good := filepath.Join(dir, "off")
	os.WriteFile(good, []byte("1700000000\n"), 0o644)
	if !near(correctedNow(good), time.Now().Add(1700000000*time.Second)) {
		t.Error("offset not applied")
	}
}

// The unit's clock is at 1970: the handshake is stamped with corrected time,
// so without the offset it is "in the future" and must read -1, not an age.
func TestHandshakeAgeUsesOffsetFile(t *testing.T) {
	const offset = 1700000000
	hs := time.Now().Add(offset*time.Second - 30*time.Second).UTC().Format(time.RFC3339)
	body := `{"BackendState":"Running","Peer":{"k":{"Online":true,"LastHandshake":"` + hs + `"}}}`

	sock := filepath.Join(t.TempDir(), "ts.sock")
	l, err := net.Listen("unix", sock)
	if err != nil {
		t.Skip("unix sockets unavailable:", err)
	}
	srv := &http.Server{Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/localapi/v0/status" {
			http.NotFound(w, r)
			return
		}
		io.WriteString(w, body)
	})}
	go srv.Serve(l)
	defer srv.Close()

	off := filepath.Join(t.TempDir(), "off")
	os.WriteFile(off, []byte("1700000000\n"), 0o644)

	c := newCache()
	pollOnce(c, sock, off, 2*time.Second)
	out := c.render(time.Now())
	if !strings.Contains(out, "state=Running\n") {
		t.Fatalf("poll failed: %q", out)
	}
	var age int
	for _, ln := range strings.Split(out, "\n") {
		if v, ok := strings.CutPrefix(ln, "last_handshake_age_s="); ok {
			age, _ = strconv.Atoi(v)
		}
	}
	if age < 28 || age > 33 {
		t.Fatalf("age with offset = %d, want ~30 (%q)", age, out)
	}

	pollOnce(c, sock, "", 2*time.Second) // 1970 clock, offset unknown
	if !strings.Contains(c.render(time.Now()), "last_handshake_age_s=-1\n") {
		t.Fatalf("no offset: want -1, got %q", c.render(time.Now()))
	}
}
