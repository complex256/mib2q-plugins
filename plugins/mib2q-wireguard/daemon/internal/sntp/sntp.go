// Package sntp fetches network time (RFC 4330 client) and sets the system
// clock. The MHI2q's RTC boots at 1970; WireGuard handshake initiations carry
// a TAI64N timestamp and a responder drops any initiation not newer than the
// last one it accepted from that peer, so after the server has seen one
// correctly-timed handshake, a unit running at 1970 can never reconnect.
package sntp

import (
	"context"
	"encoding/binary"
	"fmt"
	"net"
	"time"
)

// ntpEpochOffset is seconds from 1900-01-01 (NTP era 0) to 1970-01-01.
const ntpEpochOffset = 2208988800

// Query asks server (host or host:port) for the time. It returns the server
// time corrected by half the round trip.
func Query(ctx context.Context, server string) (time.Time, error) {
	if _, _, err := net.SplitHostPort(server); err != nil {
		server = net.JoinHostPort(server, "123")
	}
	var d net.Dialer
	c, err := d.DialContext(ctx, "udp", server)
	if err != nil {
		return time.Time{}, err
	}
	defer c.Close()
	if dl, ok := ctx.Deadline(); ok {
		c.SetDeadline(dl)
	} else {
		c.SetDeadline(time.Now().Add(5 * time.Second))
	}

	req := make([]byte, 48)
	req[0] = 0x23 // LI=0, VN=4, Mode=3 (client)
	// Transmit timestamp doubles as a nonce: the reply must echo it.
	sent := time.Now()
	binary.BigEndian.PutUint64(req[40:], toNTP(sent))
	if _, err := c.Write(req); err != nil {
		return time.Time{}, err
	}
	resp := make([]byte, 48)
	n, err := c.Read(resp)
	if err != nil {
		return time.Time{}, err
	}
	rtt := time.Since(sent)
	if n < 48 {
		return time.Time{}, fmt.Errorf("short reply (%d bytes)", n)
	}
	if mode := resp[0] & 7; mode != 4 {
		return time.Time{}, fmt.Errorf("reply mode %d, want 4 (server)", mode)
	}
	if resp[1] == 0 {
		return time.Time{}, fmt.Errorf("kiss-of-death %q", resp[12:16])
	}
	if binary.BigEndian.Uint64(resp[24:]) != binary.BigEndian.Uint64(req[40:]) {
		return time.Time{}, fmt.Errorf("reply does not match request")
	}
	tx := fromNTP(binary.BigEndian.Uint64(resp[40:]))
	return tx.Add(rtt / 2), nil
}

func toNTP(t time.Time) uint64 {
	secs := uint64(t.Unix()) + ntpEpochOffset
	frac := uint64(t.Nanosecond()) << 32 / 1e9
	return secs<<32 | frac
}

func fromNTP(v uint64) time.Time {
	secs := int64(v>>32) - ntpEpochOffset
	// Era 1 starts 2036-02-07; a small value there means we have wrapped.
	if secs < 0 {
		secs += 1 << 32
	}
	nsec := int64((v & 0xffffffff) * 1e9 >> 32)
	return time.Unix(secs, nsec)
}

// Sync tries each server in order and sets the clock from the first answer.
// It returns the adjustment applied.
func Sync(ctx context.Context, servers []string) (time.Duration, error) {
	var errs []error
	for _, s := range servers {
		qctx, cancel := context.WithTimeout(ctx, 5*time.Second)
		t, err := Query(qctx, s)
		cancel()
		if err != nil {
			errs = append(errs, fmt.Errorf("%s: %w", s, err))
			continue
		}
		delta := time.Until(t)
		if err := setClock(t); err != nil {
			return 0, fmt.Errorf("set clock: %w", err)
		}
		return delta, nil
	}
	return 0, fmt.Errorf("no NTP server answered: %v", errs)
}

// ClockPlausible reports whether the clock looks set (not the RTC's 1970).
func ClockPlausible() bool { return time.Now().Year() >= 2024 }
