#pragma once

#include <cstddef>

#include <adf.h>

namespace tapa {
namespace aie {

// One async, linear, user-locked frame of N elements, spelled with the
// buffer API of the ADF release compiling the graph.
#if __has_include(<adf/io_buffer/io_buffer_types.h>)
template <typename T, size_t N>
using input_frame = adf::input_async_buffer<T, adf::extents<N>>;
template <typename T, size_t N>
using output_frame = adf::output_async_buffer<T, adf::extents<N>>;
#else
template <typename T, size_t N>
using input_frame = adf::input_buffer<
    T, adf::buffer_port_config<adf::buffer_port_dims<N>,
                               adf::buffer_port_locking_mode::kUser,
                               adf::buffer_port_addressing_mode::kLinear, 0>>;
template <typename T, size_t N>
using output_frame = adf::output_buffer<
    T, adf::buffer_port_config<adf::buffer_port_dims<N>,
                               adf::buffer_port_locking_mode::kUser,
                               adf::buffer_port_addressing_mode::kLinear, 0>>;
#endif

}  // namespace aie
}  // namespace tapa
