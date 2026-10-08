package cram

import (
	"bytes"
	"errors"
	"io"
	"sync"
	"testing"
)

func TestOwnership(t *testing.T) {
	b, err := NewBuffer(64, 64, ReadHeavy)
	if err != nil {
		t.Fatal(err)
	}
	alias := *b
	payload := []byte("hello native allocation")
	if n, err := b.WriteAt(payload, 3); n != len(payload) || err != nil {
		t.Fatal(n, err)
	}
	got := make([]byte, len(payload))
	if _, err := b.ReadAt(got, 3); err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(payload, got) {
		t.Fatal(got)
	}
	if err := alias.Close(); err != nil {
		t.Fatal(err)
	}
	if err := b.Close(); err != nil {
		t.Fatal(err)
	}
	if _, err := b.ReadAt(got, 0); !errors.Is(err, ErrClosed) {
		t.Fatal(err)
	}
}
func TestBounds(t *testing.T) {
	for _, args := range [][3]int{{0, 64, 0}, {8, 0, 0}, {8, 3, 0}, {8, 8, 3}, {-1, 8, 0}} {
		if b, err := NewBuffer(args[0], args[1], AccessHint(args[2])); err == nil {
			b.Close()
			t.Fatal(args)
		}
	}
	b, err := NewBuffer(8, 64, Auto)
	if err != nil {
		t.Fatal(err)
	}
	defer b.Close()
	got := make([]byte, 9)
	if n, err := b.ReadAt(got, 0); n != 8 || err != io.EOF {
		t.Fatal(n, err)
	}
	if !bytes.Equal(got, make([]byte, 9)) {
		t.Fatal("not initialized")
	}
	if n, err := b.WriteAt(got, 0); n != 8 || err != io.ErrShortWrite {
		t.Fatal(n, err)
	}
	if _, err := b.ReadAt(got, -1); err == nil {
		t.Fatal("negative offset")
	}
}
func TestConcurrentClose(t *testing.T) {
	b, err := NewBuffer(128, 64, WriteHeavy)
	if err != nil {
		t.Fatal(err)
	}
	var wg sync.WaitGroup
	for i := 0; i < 8; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for j := 0; j < 100; j++ {
				_, err := b.WriteAt([]byte{42}, 0)
				if err != nil && !errors.Is(err, ErrClosed) {
					t.Error(err)
				}
				_, err = b.ReadAt(make([]byte, 1), 0)
				if err != nil && !errors.Is(err, ErrClosed) {
					t.Error(err)
				}
			}
		}()
	}
	if err := b.Close(); err != nil {
		t.Fatal(err)
	}
	wg.Wait()
}
