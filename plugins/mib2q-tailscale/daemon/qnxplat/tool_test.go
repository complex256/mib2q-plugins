package qnxplat

import "testing"

func TestToolFallsBackToBareName(t *testing.T) {
	if got := Tool("definitely-not-a-real-tool"); got != "definitely-not-a-real-tool" {
		t.Errorf("Tool = %q, want the bare name when nothing is found", got)
	}
}
