#include "libcram.h"
#include <assert.h>
#include <pthread.h>
#include <stdint.h>
#include <string.h>
static void* worker(void* arg) {
    (void)arg;
    assert(cram_init()==CRAM_OK);
    for (int i=0;i<100;i++) {
        void* p=cram_alloc(4096,256,CRAM_WRITE_HEAVY);
        assert(p && (uintptr_t)p%256==0);
        memset(p,42,4096);
        assert(cram_free(p)==CRAM_OK);
    }
    assert(cram_last_error()==CRAM_OK);
    return NULL;
}
int main(void) {
    assert(cram_init()==CRAM_OK);
    cram_telemetry before, after;
    assert(cram_get_telemetry(&before)==CRAM_OK);
    void* p=cram_alloc(1024,64,CRAM_READ_HEAVY);
    assert(p);
    assert(cram_get_telemetry(&after)==CRAM_OK);
    assert(after.allocation_count==before.allocation_count+1);
    assert(after.allocated_bytes>=before.allocated_bytes+1024);
    assert(cram_free(p)==CRAM_OK);
    assert(cram_free(NULL)==CRAM_OK);
    int foreign=0;
    assert(cram_free(&foreign)!=CRAM_OK);
    assert(cram_alloc(1,3,0)==NULL);
    pthread_t threads[4];
    for(int i=0;i<4;i++) assert(pthread_create(&threads[i],NULL,worker,NULL)==0);
    for(int i=0;i<4;i++) assert(pthread_join(threads[i],NULL)==0);
    assert(cram_last_error()==CRAM_INVALID_ARGUMENT);
    assert(cram_register_pressure_callback(NULL,NULL)==CRAM_OK);
    assert(cram_get_telemetry(&after)==CRAM_OK);
    assert(after.allocation_count==before.allocation_count);
    assert(after.allocated_bytes==before.allocated_bytes);
    return 0;
}
