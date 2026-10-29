package proxy

import (
	"context"
	"io"
	"net"
	"net/http"
	"time"

	"github.com/complex256/mib2q-wireguard/internal/forward"
)

// NewHTTP returns an HTTP proxy handler: CONNECT tunnels for HTTPS and
// absolute-URI forwarding for plain HTTP, both dialed through d.
func NewHTTP(d Dialer, logf func(string, ...any)) http.Handler {
	tr := &http.Transport{
		DialContext:         d.DialContext,
		Proxy:               nil,
		MaxIdleConns:        16,
		IdleConnTimeout:     60 * time.Second,
		TLSHandshakeTimeout: 15 * time.Second,
	}
	return &httpProxy{d: d, tr: tr, logf: logf}
}

type httpProxy struct {
	d    Dialer
	tr   *http.Transport
	logf func(string, ...any)
}

// Hop-by-hop headers, RFC 9110 7.6.1.
var hopHeaders = []string{
	"Connection", "Proxy-Connection", "Keep-Alive", "Proxy-Authenticate",
	"Proxy-Authorization", "Te", "Trailer", "Transfer-Encoding", "Upgrade",
}

func (p *httpProxy) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	if r.Method == http.MethodConnect {
		p.connect(w, r)
		return
	}
	if r.URL.Host == "" {
		http.Error(w, "mibwg: this is a proxy; send an absolute URI", http.StatusBadRequest)
		return
	}
	out := r.Clone(r.Context())
	out.RequestURI = ""
	for _, h := range hopHeaders {
		out.Header.Del(h)
	}
	resp, err := p.tr.RoundTrip(out)
	if err != nil {
		p.logf("http: %s %s: %v", r.Method, r.URL, err)
		http.Error(w, err.Error(), http.StatusBadGateway)
		return
	}
	defer resp.Body.Close()
	for _, h := range hopHeaders {
		resp.Header.Del(h)
	}
	for k, vs := range resp.Header {
		for _, v := range vs {
			w.Header().Add(k, v)
		}
	}
	w.WriteHeader(resp.StatusCode)
	io.Copy(w, resp.Body)
}

func (p *httpProxy) connect(w http.ResponseWriter, r *http.Request) {
	ctx, cancel := context.WithTimeout(r.Context(), dialTimeout)
	up, err := p.d.DialContext(ctx, "tcp", r.Host)
	cancel()
	if err != nil {
		p.logf("http: CONNECT %s: %v", r.Host, err)
		http.Error(w, err.Error(), http.StatusBadGateway)
		return
	}
	defer up.Close()
	hj, ok := w.(http.Hijacker)
	if !ok {
		http.Error(w, "hijack unsupported", http.StatusInternalServerError)
		return
	}
	c, brw, err := hj.Hijack()
	if err != nil {
		return
	}
	defer c.Close()
	if _, err := c.Write([]byte("HTTP/1.1 200 Connection established\r\n\r\n")); err != nil {
		return
	}
	forward.Pipe(&bufferedConn{c, brw.Reader}, up)
}

// ServeHTTPProxy serves until ln is closed.
func ServeHTTPProxy(ctx context.Context, ln net.Listener, h http.Handler) error {
	srv := &http.Server{Handler: h, ReadHeaderTimeout: 30 * time.Second}
	go func() {
		<-ctx.Done()
		srv.Close()
	}()
	err := srv.Serve(ln)
	if ctx.Err() != nil {
		return nil
	}
	return err
}
