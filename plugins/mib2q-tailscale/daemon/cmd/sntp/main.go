// sntp sets the unit's clock from network time. The MHI2q's RTC boots at
// 1970, and TLS to the Tailscale control plane fails against a 1970 clock.
package main

import (
	"encoding/binary"
	"fmt"
	"net"
	"os"
	"time"
)

const ntpEpochOffset = 2208988800

// minPlausibleYear guards ClockSettime the same way
// mib2q-wireguard/internal/sntp's ClockPlausible does: a reply claiming a
// year before this is treated as bad data (a misbehaving or spoofed
// server), not a real time, and is never applied.
const minPlausibleYear = 2024

// attemptsPerServer bounds retries per server: one early DNS failure at
// boot (the uplink isn't up yet) shouldn't cost the whole drive, but this
// must still return in bounded time.
const attemptsPerServer = 3

const retryDelay = 2 * time.Second

var defaultServers = []string{"pool.ntp.org", "time.google.com"}

func main() {
	// -print reports the offset and leaves the clock alone; it is what the boot
	// path uses. Stepping the clock while the HMI is up fires its pending timers
	// and restarts the navigation domain, so the legacy path below is now only
	// for setting the clock by hand.
	for _, a := range os.Args[1:] {
		if a == "-print" {
			os.Exit(run(os.Args[1:], deps{
				now:    time.Now,
				query:  query,
				out:    os.Stdout,
				errOut: os.Stderr,
			}))
		}
	}

	servers := os.Args[1:]
	if len(servers) == 0 {
		servers = defaultServers
	}

	if time.Now().Year() >= minPlausibleYear {
		fmt.Println("sntp: clock already plausible, leaving it alone")
		return
	}

	for _, s := range servers {
		var t time.Time
		var err error
		for attempt := 1; attempt <= attemptsPerServer; attempt++ {
			t, err = query(s)
			if err == nil {
				break
			}
			fmt.Printf("sntp: %s: attempt %d/%d: %v\n", s, attempt, attemptsPerServer, err)
			if attempt < attemptsPerServer {
				time.Sleep(retryDelay)
			}
		}
		if err != nil {
			continue
		}
		if t.Year() < minPlausibleYear {
			fmt.Printf("sntp: %s: implausible reply %s, ignoring\n", s, t.UTC().Format(time.RFC3339))
			continue
		}
		if err := setClock(t); err != nil {
			fmt.Println("sntp: set clock:", err)
			os.Exit(1)
		}
		fmt.Println("sntp: clock set to", t.UTC().Format(time.RFC3339))
		return
	}
	fmt.Println("sntp: no server answered with a plausible time")
	os.Exit(1)
}

func query(server string) (time.Time, error) {
	if _, _, err := net.SplitHostPort(server); err != nil {
		server = net.JoinHostPort(server, "123")
	}
	c, err := net.DialTimeout("udp", server, 5*time.Second)
	if err != nil {
		return time.Time{}, err
	}
	defer c.Close()
	c.SetDeadline(time.Now().Add(5 * time.Second))
	req := make([]byte, 48)
	req[0] = 0x23 // LI=0, VN=4, Mode=3 (client)
	sent := time.Now()
	binary.BigEndian.PutUint64(req[40:], uint64(sent.Unix()+ntpEpochOffset)<<32)
	if _, err := c.Write(req); err != nil {
		return time.Time{}, err
	}
	resp := make([]byte, 48)
	n, err := c.Read(resp)
	if err != nil {
		return time.Time{}, err
	}
	if n < 48 {
		return time.Time{}, fmt.Errorf("short reply (%d bytes)", n)
	}
	if mode := resp[0] & 7; mode != 4 {
		return time.Time{}, fmt.Errorf("reply mode %d, want 4", mode)
	}
	if resp[1] == 0 {
		return time.Time{}, fmt.Errorf("kiss-of-death")
	}
	secs := int64(binary.BigEndian.Uint32(resp[40:44])) - ntpEpochOffset
	frac := int64(binary.BigEndian.Uint32(resp[44:48])) * 1e9 >> 32
	return time.Unix(secs, frac).Add(time.Since(sent) / 2), nil
}
