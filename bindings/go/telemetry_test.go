package cram

import "testing"

func TestTelemetry(t *testing.T) {
	if err := Init(); err != nil {
		t.Fatal(err)
	}
	before, err := Telemetry()
	if err != nil {
		t.Fatal(err)
	}
	b, err := NewBuffer(512, 64, Auto)
	if err != nil {
		t.Fatal(err)
	}
	after, err := Telemetry()
	if err != nil {
		t.Fatal(err)
	}
	if after.AllocationCount != before.AllocationCount+1 || after.AllocatedBytes < before.AllocatedBytes+512 {
		t.Fatal(before, after)
	}
	if err := b.Close(); err != nil {
		t.Fatal(err)
	}
	final, err := Telemetry()
	if err != nil {
		t.Fatal(err)
	}
	if final.AllocationCount != before.AllocationCount {
		t.Fatal(final)
	}
}
