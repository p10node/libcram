#ifndef CRAM_RESOURCE_HPP
#define CRAM_RESOURCE_HPP
#include "libcram.hpp"
namespace cram::pmr {
using memory_resource = cram::memory_resource;
template<class T>
using polymorphic_allocator = std::pmr::polymorphic_allocator<T>;
}
#endif
