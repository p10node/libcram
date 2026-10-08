import gc
import pytest
import libcram

def test_owned_buffer_survives_pool_close_and_gc():
    pool = libcram.MemoryPool()
    buf = pool.allocate(64, alignment=64)
    view = memoryview(buf)
    assert view.format == "B" and view.nbytes == 64
    assert bytes(view) == bytes(64)
    view[:] = bytes(range(64))
    pool.close()
    del pool, buf
    gc.collect()
    assert view[63] == 63
    view.release()


def test_close_export_slices_and_contexts():
    with libcram.MemoryPool() as pool:
        buf = pool.allocate(16)
        first = memoryview(buf)
        second = first[2:]
        with pytest.raises(BufferError):
            buf.close()
        first.release()
        with pytest.raises(BufferError):
            buf.close()
        second.release()
        buf.close()
        buf.close()
        assert buf.closed
        with pytest.raises(BufferError):
            memoryview(buf)
        with pool.allocate(8) as other:
            assert len(other) == 8
        assert other.closed
    assert pool.closed
    with pytest.raises(RuntimeError):
        pool.allocate(8)

@pytest.mark.parametrize("args", [(0, 64, 0), (1, 0, 0), (1, 3, 0), (1, 64, 3), (2**63, 64, 0)])
def test_invalid(args):
    with pytest.raises(ValueError):
        libcram.MemoryPool().allocate(args[0], alignment=args[1], hint=args[2])

def test_concurrent_pool_lifecycle():
    from concurrent.futures import ThreadPoolExecutor
    pool = libcram.MemoryPool()
    def worker(index):
        for _ in range(100):
            try:
                buf = pool.allocate(128)
            except RuntimeError:
                assert pool.closed
                return
            with memoryview(buf) as view:
                view[0] = index
                assert view[0] == index
            buf.close()
    with ThreadPoolExecutor(max_workers=8) as executor:
        jobs = [executor.submit(worker, n) for n in range(8)]
        pool.close()
        for job in jobs:
            job.result()


def test_cross_thread_close_rejects_live_export():
    from concurrent.futures import ThreadPoolExecutor
    pool = libcram.MemoryPool()
    buf = pool.allocate(64)
    view = memoryview(buf)
    with ThreadPoolExecutor(max_workers=1) as executor:
        with pytest.raises(BufferError):
            executor.submit(buf.close).result()
    view[0] = 91
    assert view[0] == 91
    view.release()
    buf.close()
    assert buf.closed


def test_corrupted_canary_close_still_consumes_native_owner():
    import ctypes
    import pytest
    import libcram
    buffer = libcram.MemoryPool().allocate(64)
    view = memoryview(buffer)
    address = ctypes.addressof(ctypes.c_char.from_buffer(view))
    view.release()
    # The extra canary is inside the mapping exclusively owned by this fixture.
    ctypes.c_ubyte.from_address(address + 64).value = 0
    with pytest.raises(RuntimeError):
        buffer.close()
    with pytest.raises(BufferError):
        memoryview(buffer)
    buffer.close()
