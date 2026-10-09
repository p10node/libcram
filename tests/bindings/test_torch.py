import gc
import pytest
import torch
import libcram


def test_cpu_kv_tensor_keeps_export_alive_across_tensor_views():
    pool = libcram.MemoryPool()
    buffer = pool.allocate(4096)
    tensor = buffer.tensor(torch.float32)
    kv = tensor.reshape(2, 4, 128)
    tensor.fill_(7.25)
    assert kv[1, 3, 127].item() == 7.25
    assert tensor.data_ptr() % 64 == 0
    pool.close()
    with pytest.raises(BufferError):
        buffer.close()
    del tensor
    gc.collect()
    with pytest.raises(BufferError):
        buffer.close()
    assert kv[0, 0, 0].item() == 7.25
    del kv
    gc.collect()
    buffer.close()
    assert buffer.closed


def test_distinct_cpu_tensor_storage_and_dtype_validation():
    pool = libcram.MemoryPool()
    a, b = pool.allocate(128), pool.allocate(128)
    ta, tb = a.tensor(torch.int16), b.tensor(torch.int16)
    ta.fill_(13)
    tb.fill_(29)
    assert ta.data_ptr() != tb.data_ptr()
    assert ta.tolist() == [13] * 64
    assert tb.tolist() == [29] * 64
    with pytest.raises(TypeError):
        a.tensor("not-a-dtype")
    del ta, tb
    gc.collect()
    a.close()
    b.close()
