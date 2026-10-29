// mibwg is a WireGuard client for the Audi/VW MHI2q (MIB2) head unit - QNX
// 6.5.0 on ARMv7 - built on wireguard-go with a userspace TCP/IP stack,
// because the unit's io-pkt network stack has no tun device.
package main

import (
	"bufio"
	"context"
	"flag"
	"fmt"
	"io"
	"log"
	"os"
	"os/signal"
	"strings"
	"syscall"

	"github.com/complex256/mib2q-wireguard/internal/config"
	"github.com/complex256/mib2q-wireguard/internal/daemon"
	"github.com/complex256/mib2q-wireguard/internal/keys"
)

var version = "dev"

const usage = `mibwg - WireGuard for the MHI2q head unit (QNX 6.5, userspace netstack)

usage:
  mibwg up [-v] [-status FILE] [-log FILE] CONFIG   run the tunnel (foreground)
  mibwg check CONFIG                                parse and validate CONFIG
  mibwg genkey                                      print a new private key
  mibwg pubkey < private.key                        print the matching public key
  mibwg version
`

func main() {
	if len(os.Args) < 2 {
		fmt.Fprint(os.Stderr, usage)
		os.Exit(2)
	}
	var err error
	switch cmd, args := os.Args[1], os.Args[2:]; cmd {
	case "up":
		err = up(args)
	case "check":
		err = check(args)
	case "genkey":
		err = genkey()
	case "pubkey":
		err = pubkey(os.Stdin)
	case "version", "-version", "--version":
		fmt.Println("mibwg", version)
	case "help", "-h", "-help", "--help":
		fmt.Print(usage)
	default:
		fmt.Fprintf(os.Stderr, "mibwg: unknown command %q\n\n%s", cmd, usage)
		os.Exit(2)
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "mibwg:", err)
		os.Exit(1)
	}
}

func up(args []string) error {
	fs := flag.NewFlagSet("up", flag.ExitOnError)
	verbose := fs.Bool("v", false, "verbose WireGuard logging")
	status := fs.String("status", "", "rewrite peer status to this file every 30s")
	logPath := fs.String("log", "", "append log to this file instead of stderr")
	fs.Parse(args)
	if fs.NArg() != 1 {
		return fmt.Errorf("up: need exactly one CONFIG argument")
	}
	cfg, err := config.Load(fs.Arg(0))
	if err != nil {
		return err
	}
	out := io.Writer(os.Stderr)
	if *logPath != "" {
		f, err := os.OpenFile(*logPath, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o644)
		if err != nil {
			return err
		}
		defer f.Close()
		out = f
	}
	logger := log.New(out, "mibwg: ", log.LstdFlags|log.LUTC)
	logger.Printf("starting %s, pid %d", version, os.Getpid())

	// Started in the background from an SSH shell; the unit has no nohup.
	signal.Ignore(syscall.SIGHUP)
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	d, err := daemon.Start(ctx, cfg, daemon.Options{Logf: logger.Printf, Verbose: *verbose, Status: *status})
	if err != nil {
		return err
	}
	select {
	case <-ctx.Done():
		logger.Printf("signal received, shutting down")
	case <-d.Wait():
		logger.Printf("device closed")
	}
	d.Close()
	return nil
}

func check(args []string) error {
	if len(args) != 1 {
		return fmt.Errorf("check: need exactly one CONFIG argument")
	}
	cfg, err := config.Load(args[0])
	if err != nil {
		return err
	}
	pub, err := keys.Public(cfg.Interface.PrivateKey)
	if err != nil {
		return err
	}
	fmt.Printf("ok: this unit's public key is %s\n", pub)
	fmt.Printf("  addresses %v, %d peer(s), %d forward(s)\n", cfg.Interface.Addresses, len(cfg.Peers), len(cfg.Forwards))
	for _, fw := range cfg.Forwards {
		fmt.Printf("  forward %s %d -> %s\n", fw.Proto, fw.ListenPort, fw.Target)
	}
	if cfg.Proxy.SOCKS5 != "" {
		fmt.Printf("  socks5 proxy on %s\n", cfg.Proxy.SOCKS5)
	}
	if cfg.Proxy.HTTP != "" {
		fmt.Printf("  http proxy on %s\n", cfg.Proxy.HTTP)
	}
	return nil
}

func genkey() error {
	k, err := keys.NewPrivate()
	if err != nil {
		return err
	}
	fmt.Println(k)
	return nil
}

func pubkey(r io.Reader) error {
	line, err := bufio.NewReader(r).ReadString('\n')
	if err != nil && line == "" {
		return fmt.Errorf("pubkey: read private key from stdin: %w", err)
	}
	priv, err := config.ParseKey(strings.TrimSpace(line))
	if err != nil {
		return err
	}
	pub, err := keys.Public(priv)
	if err != nil {
		return err
	}
	fmt.Println(pub)
	return nil
}
