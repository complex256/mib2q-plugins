package sntp

import (
	"context"
	"encoding/binary"
	"net"
	"testing"
	"time"
)

// fakeServer answers like an NTP server whose clock reads `now`.
func fakeServer(t *testing.T, now time.Time, mutate func([]byte)) string {
	pc, err := net.ListenPacket("udp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { pc.Close() })
	go func() {
		buf := make([]byte, 48)
		for {
			n, from, err := pc.ReadFrom(buf)
			if err != nil || n < 48 {
				return
			}
			resp := make([]byte, 48)
			resp[0] = 0x24 // VN=4, mode 4
			resp[1] = 2    // stratum
			copy(resp[24:32], buf[40:48])
			binary.BigEndian.PutUint64(resp[40:], toNTP(now))
			if mutate != nil {
				mutate(resp)
			}
			pc.WriteTo(resp, from)
		}
	}()
	return pc.LocalAddr().String()
}

func TestQuery(t *testing.T) {
	want := time.Date(2026, 9, 18, 12, 0, 0, 500_000_000, time.UTC)
	got, err := Query(context.Background(), fakeServer(t, want, nil))
	if err != nil {
		t.Fatal(err)
	}
	if d := got.Sub(want); d < 0 || d > time.Second {
		t.Fatalf("got %v, want ~%v", got, want)
	}
}

func TestQueryRejects(t *testing.T) {
	now := time.Now()
	for name, mutate := range map[string]func([]byte){
		"kiss of death": func(b []byte) { b[1] = 0 },
		"wrong mode":    func(b []byte) { b[0] = 0x23 },
		"stale nonce":   func(b []byte) { b[24]++ },
	} {
		if _, err := Query(context.Background(), fakeServer(t, now, mutate)); err == nil {
			t.Errorf("%s: accepted", name)
		}
	}
}

func TestNTPRoundTrip(t *testing.T) {
	for _, tm := range []time.Time{
		time.Date(1970, 1, 1, 0, 0, 1, 0, time.UTC),
		time.Date(2026, 9, 18, 0, 0, 0, 123456789, time.UTC),
		time.Date(2035, 12, 31, 23, 59, 59, 0, time.UTC),
	} {
		if got := fromNTP(toNTP(tm)); got.Sub(tm).Abs() > time.Microsecond {
			t.Errorf("%v -> %v", tm, got)
		}
	}
}
