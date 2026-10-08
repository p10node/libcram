package cram

/*
#include "libcram.h"
*/
import "C"

// Init initializes the singleton; NewBuffer also initializes it lazily.
func Init() error {
	if status := C.cram_init(); status != C.CRAM_OK {
		return StatusError(status)
	}
	return nil
}

type TelemetrySnapshot struct {
	AllocatedBytes  uint64
	CramBytes       uint64
	AllocationCount uint64
	PressureLevel   uint32
	PSISomeAvg10    float64
	PSIFullAvg10    float64
}

func Telemetry() (TelemetrySnapshot, error) {
	var t C.cram_telemetry
	if status := C.cram_get_telemetry(&t); status != C.CRAM_OK {
		return TelemetrySnapshot{}, StatusError(status)
	}
	return TelemetrySnapshot{uint64(t.allocated_bytes), uint64(t.cram_bytes), uint64(t.allocation_count), uint32(t.pressure_level), float64(t.psi_some_avg10), float64(t.psi_full_avg10)}, nil
}
