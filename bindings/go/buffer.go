// Package cram provides closeable native allocations. No Go pointer is retained
// by C. ReadAt and WriteAt copy bytes under a shared ownership lock; no borrowed
// native slice escapes, so Close is safe against concurrent reads and writes.
package cram

/*
#cgo CFLAGS: -I${SRCDIR}/../../include
#cgo LDFLAGS: -lcram
#include "libcram.h"
// Capture TLS error in the same cgo call: a goroutine may change OS threads
// between separate calls to cram_alloc and cram_last_error.
static void* cram_go_alloc(size_t size, size_t alignment, uint32_t hint, int32_t* status) {
    void* p = cram_alloc(size, alignment, hint);
    *status = cram_last_error();
    return p;
}
*/
import "C"
import (
	"errors"
	"fmt"
	"io"
	"runtime"
	"sync"
	"unsafe"
)

type AccessHint uint32

const (
	Auto AccessHint = iota
	ReadHeavy
	WriteHeavy
)

var ErrClosed = errors.New("libcram: buffer closed")
var ErrArgument = errors.New("libcram: invalid argument")

type StatusError int32

func (s StatusError) Error() string { return fmt.Sprintf("libcram: status %d", s) }

type storage struct {
	mu   sync.Mutex
	ptr  unsafe.Pointer
	size int
}

// Buffer may be copied: copies share the same lock and close state. Prefer a
// pointer. Its zero value is closed. Call Close deterministically; the finalizer
// is only a fallback and does not give bounded release latency.
type Buffer struct{ owner *storage }

func NewBuffer(size, alignment int, hint AccessHint) (*Buffer, error) {
	if size <= 0 || alignment <= 0 || alignment&(alignment-1) != 0 || hint > WriteHeavy {
		return nil, ErrArgument
	}
	var status C.int32_t
	p := C.cram_go_alloc(C.size_t(size), C.size_t(alignment), C.uint32_t(hint), &status)
	if p == nil {
		return nil, StatusError(status)
	}
	s := &storage{ptr: p, size: size}
	clear(unsafe.Slice((*byte)(p), size))
	runtime.SetFinalizer(s, (*storage).finalize)
	return &Buffer{owner: s}, nil
}
func (s *storage) finalize() { _ = s.close() }
func (s *storage) close() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.ptr == nil {
		return nil
	}
	ptr := s.ptr
	s.ptr = nil
	s.size = 0
	// Guard corruption is reported after native ownership has been consumed.
	status := C.cram_free(ptr)
	if status != C.CRAM_OK {
		return StatusError(status)
	}
	return nil
}
func (b *Buffer) Close() error {
	if b == nil || b.owner == nil {
		return nil
	}
	err := b.owner.close()
	runtime.KeepAlive(b)
	return err
}
func (b *Buffer) ReadAt(dst []byte, offset int64) (int, error) {
	if b == nil || b.owner == nil {
		return 0, ErrClosed
	}
	s := b.owner
	s.mu.Lock()
	defer s.mu.Unlock()
	defer runtime.KeepAlive(b)
	if s.ptr == nil {
		return 0, ErrClosed
	}
	if offset < 0 {
		return 0, ErrArgument
	}
	if offset > int64(s.size) {
		return 0, io.EOF
	}
	n := copy(dst, unsafe.Slice((*byte)(s.ptr), s.size)[int(offset):])
	if n < len(dst) {
		return n, io.EOF
	}
	return n, nil
}
func (b *Buffer) WriteAt(src []byte, offset int64) (int, error) {
	if b == nil || b.owner == nil {
		return 0, ErrClosed
	}
	s := b.owner
	s.mu.Lock()
	defer s.mu.Unlock()
	defer runtime.KeepAlive(b)
	if s.ptr == nil {
		return 0, ErrClosed
	}
	if offset < 0 {
		return 0, ErrArgument
	}
	if offset > int64(s.size) {
		return 0, io.ErrShortWrite
	}
	n := copy(unsafe.Slice((*byte)(s.ptr), s.size)[int(offset):], src)
	if n < len(src) {
		return n, io.ErrShortWrite
	}
	return n, nil
}
