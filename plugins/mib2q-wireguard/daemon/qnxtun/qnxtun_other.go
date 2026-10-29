//go:build !qnx

package qnxtun

import "errors"

// Device exists on every platform so callers compile; only QNX can open one.
type Device struct{}

func Open(string) (*Device, error)        { return nil, errors.New("qnxtun: QNX only") }
func (*Device) Read([]byte) (int, error)  { return 0, errors.New("qnxtun: QNX only") }
func (*Device) Write([]byte) (int, error) { return 0, errors.New("qnxtun: QNX only") }
func (*Device) Close() error              { return nil }
func (*Device) Fd() int                   { return -1 }
func (*Device) Name() string              { return "" }
