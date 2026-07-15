#ifndef TAPA_HOST_FRT_C_API_H_
#define TAPA_HOST_FRT_C_API_H_

#include <cstddef>
#include <cstdint>

// Single declaration point for the FRT C ABIs implemented in Rust. The Rust
// `#[no_mangle]` definitions are the source of truth:
//   * `frt_instance_*` -> fpga-runtime/frt/src/ffi.rs
//   * `frt_shmq_*`     -> fpga-runtime/frt/src/shm_ffi.rs

extern "C" {

// -- Instance ABI (fpga-runtime/frt/src/ffi.rs) --
void* frt_instance_open(const char* path, const char* simulator);
void frt_instance_close(void* handle);
const char* frt_last_error_message();
int frt_instance_set_scalar_bytes(void* handle, uint32_t index,
                                  const uint8_t* value, size_t size);
int frt_instance_set_buffer_arg_typed(void* handle, uint32_t index,
                                      uint8_t* ptr, size_t bytes, int tag);
int frt_instance_set_stream_arg(void* handle, uint32_t index,
                                const char* shm_path);
size_t frt_instance_suspend_buffer(void* handle, uint32_t index);
int frt_instance_get_arg_count(void* handle, uint32_t* out_count);
int frt_instance_get_arg(void* handle, uint32_t ordinal, uint32_t* out_index,
                         int* out_cat, const char** out_name,
                         const char** out_type);
int frt_instance_write_to_device(void* handle);
int frt_instance_read_from_device(void* handle);
int frt_instance_exec(void* handle);
int frt_instance_pause(void* handle);
int frt_instance_resume(void* handle);
int frt_instance_finish(void* handle);
int frt_instance_kill(void* handle);
int frt_instance_is_finished(void* handle);
uint64_t frt_instance_load_ns(void* handle);
uint64_t frt_instance_compute_ns(void* handle);
uint64_t frt_instance_store_ns(void* handle);

// -- Shared-memory stream ABI (fpga-runtime/frt/src/shm_ffi.rs) --
void* frt_shmq_create(uint32_t depth, uint32_t width, char* out_path,
                      size_t out_path_len);
void frt_shmq_destroy(void* handle);
int frt_shmq_empty(const void* handle);
int frt_shmq_full(const void* handle);
int frt_shmq_push(void* handle, const uint8_t* data, size_t len);
int frt_shmq_front(const void* handle, uint8_t* out, size_t len);
int frt_shmq_pop(void* handle, uint8_t* out, size_t len);

}  // extern "C"

#endif  // TAPA_HOST_FRT_C_API_H_
