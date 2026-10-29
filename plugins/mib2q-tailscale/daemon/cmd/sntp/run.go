package main

import (
	"fmt"
	"io"
	"time"
)

// deps are the seams the tests replace: the clock and the network query.
type deps struct {
	now    func() time.Time
	query  func(string) (time.Time, error)
	out    io.Writer
	errOut io.Writer
}

// run implements -print: report how far this unit's clock is behind network
// time, in whole seconds, and leave the clock alone.
//
// stdout carries the offset and nothing else, so tailscale.sh can read it with
// a plain command substitution; every human-readable line goes to stderr.
func run(args []string, d deps) int {
	printOnly := false
	var servers []string
	for _, a := range args {
		if a == "-print" {
			printOnly = true
			continue
		}
		servers = append(servers, a)
	}
	if !printOnly {
		fmt.Fprintln(d.errOut, "sntp: run requires -print")
		return 1
	}
	if len(servers) == 0 {
		servers = defaultServers
	}
	for _, s := range servers {
		t, err := d.query(s)
		if err != nil {
			fmt.Fprintf(d.errOut, "sntp: %s: %v\n", s, err)
			continue
		}
		if t.Year() < minPlausibleYear {
			fmt.Fprintf(d.errOut, "sntp: %s: implausible reply %s, ignoring\n", s, t.UTC().Format(time.RFC3339))
			continue
		}
		fmt.Fprintln(d.out, int64(t.Sub(d.now())/time.Second))
		return 0
	}
	fmt.Fprintln(d.errOut, "sntp: no server answered with a plausible time")
	return 1
}
