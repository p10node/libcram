package cram

import (
	"testing"
	"unsafe"
)

func TestCloseConsumesPointerEvenWhenCanaryReportsCorruption(t *testing.T) {
	b, err := NewBuffer(64, 64, Auto)
	if err != nil {
		t.Fatal(err)
	}
	// The core reserves a trailing canary within this test-owned mapping.
	*(*byte)(unsafe.Add(b.owner.ptr, b.owner.size)) = 0
	if b.Close() == nil {
		t.Fatal("missing corruption status")
	}
	if b.owner.ptr != nil {
		t.Fatal("consumed native pointer retained after error")
	}
	if err := b.Close(); err != nil {
		t.Fatal(err)
	}
}
