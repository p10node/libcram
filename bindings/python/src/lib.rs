//! Owning, zero-initialized Python buffers over the C allocator.
#![deny(clippy::unwrap_used)]
use pyo3::{
    exceptions::{PyBufferError, PyRuntimeError, PyValueError},
    ffi,
    prelude::*,
};
use std::{
    ffi::c_void,
    sync::{Mutex, MutexGuard},
};
fn locked<T>(lock: &Mutex<T>) -> PyResult<MutexGuard<'_, T>> {
    lock.lock()
        .map_err(|_| PyRuntimeError::new_err("poisoned buffer lock"))
}
fn failure() -> PyErr {
    PyRuntimeError::new_err(format!("libcram status {}", cram::cram_last_error()))
}
struct Storage {
    address: usize,
    size: usize,
    exports: usize,
}
impl Storage {
    fn free(&mut self) -> PyResult<()> {
        if self.address != 0 {
            let address = std::mem::replace(&mut self.address, 0);
            // SAFETY: no exports remain and this Storage exclusively owns the mapping.
            // A canary error is returned after native ownership has been consumed.
            if unsafe { cram::cram_free(address as *mut c_void) } != cram::CRAM_OK {
                return Err(failure());
            }
        }
        Ok(())
    }
}
impl Drop for Storage {
    fn drop(&mut self) {
        let _ = self.free();
    }
}
/// The buffer owns its allocation independently of the pool. Closing is rejected
/// while a Py_buffer export exists; exports hold a strong reference to this object.
#[pyclass(frozen, module = "libcram")]
struct Buffer {
    storage: Mutex<Storage>,
}
#[pymethods]
impl Buffer {
    fn __enter__(slf: Bound<'_, Self>) -> PyResult<Bound<'_, Self>> {
        if slf.get().closed()? {
            return Err(PyBufferError::new_err("buffer is closed"));
        }
        Ok(slf)
    }
    fn __exit__(
        &self,
        _kind: &Bound<'_, PyAny>,
        _value: &Bound<'_, PyAny>,
        _trace: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        self.close()
    }
    fn close(&self) -> PyResult<()> {
        let mut s = locked(&self.storage)?;
        if s.exports != 0 {
            return Err(PyBufferError::new_err("buffer has active exports"));
        }
        s.free()
    }
    fn __len__(&self) -> PyResult<usize> {
        let s = locked(&self.storage)?;
        Ok(if s.address == 0 { 0 } else { s.size })
    }
    #[getter]
    fn closed(&self) -> PyResult<bool> {
        Ok(locked(&self.storage)?.address == 0)
    }
    /// # Safety
    /// Python supplies a valid writable Py_buffer and releases it exactly once
    /// after success. PyBuffer_FillInfo retains self until that release.
    unsafe fn __getbuffer__(
        slf: Bound<'_, Self>,
        view: *mut ffi::Py_buffer,
        flags: i32,
    ) -> PyResult<()> {
        let mut s = locked(&slf.get().storage)?;
        if s.address == 0 {
            return Err(PyBufferError::new_err("buffer is closed"));
        }
        if view.is_null() {
            return Err(PyBufferError::new_err("null Py_buffer"));
        }
        let result = unsafe {
            ffi::PyBuffer_FillInfo(
                view,
                slf.as_ptr(),
                s.address as *mut c_void,
                s.size as isize,
                0,
                flags,
            )
        };
        if result != 0 {
            return Err(PyErr::fetch(slf.py()));
        }
        if flags & ffi::PyBUF_FORMAT != 0 {
            unsafe {
                (*view).format = c"B".as_ptr().cast_mut();
            }
        }
        s.exports += 1;
        Ok(())
    }
    /// # Safety
    /// Called by Python only for a successful matching export.
    unsafe fn __releasebuffer__(&self, _view: *mut ffi::Py_buffer) {
        if let Ok(mut s) = self.storage.lock() {
            s.exports = s.exports.saturating_sub(1);
        }
    }
}
/// A closeable allocation factory, not a bulk-free arena. Existing buffers remain
/// valid after pool.close(); the factory does not retain buffers or exported views.
#[pyclass(frozen, module = "libcram")]
struct MemoryPool {
    closed: Mutex<bool>,
}
#[pymethods]
impl MemoryPool {
    fn __enter__(slf: Bound<'_, Self>) -> PyResult<Bound<'_, Self>> {
        if slf.get().closed()? {
            return Err(PyRuntimeError::new_err("pool is closed"));
        }
        Ok(slf)
    }
    fn __exit__(
        &self,
        _kind: &Bound<'_, PyAny>,
        _value: &Bound<'_, PyAny>,
        _trace: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        self.close()
    }
    #[new]
    fn new() -> PyResult<Self> {
        if cram::cram_init() != cram::CRAM_OK {
            return Err(failure());
        }
        Ok(Self {
            closed: Mutex::new(false),
        })
    }
    #[pyo3(signature=(size, alignment=64, hint=0))]
    fn allocate(&self, size: usize, alignment: usize, hint: u32) -> PyResult<Buffer> {
        let closed = locked(&self.closed)?;
        if *closed {
            return Err(PyRuntimeError::new_err("pool is closed"));
        }
        if size == 0
            || size > isize::MAX as usize
            || std::alloc::Layout::from_size_align(size, alignment).is_err()
            || hint > 2
        {
            return Err(PyValueError::new_err(
                "invalid size, alignment, or access hint",
            ));
        }
        let ptr = cram::cram_alloc(size, alignment, hint);
        if ptr.is_null() {
            return Err(failure());
        }
        // Never expose uninitialized native bytes to Python.
        unsafe {
            ptr.cast::<u8>().write_bytes(0, size);
        }
        Ok(Buffer {
            storage: Mutex::new(Storage {
                address: ptr as usize,
                size,
                exports: 0,
            }),
        })
    }
    fn close(&self) -> PyResult<()> {
        *locked(&self.closed)? = true;
        Ok(())
    }
    #[getter]
    fn closed(&self) -> PyResult<bool> {
        Ok(*locked(&self.closed)?)
    }
}
#[pymodule(gil_used = true)]
fn libcram(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<MemoryPool>()?;
    m.add_class::<Buffer>()?;
    Ok(())
}
