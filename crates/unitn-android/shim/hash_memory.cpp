// Skia's prebuilt library (pulled in by Slint 1.20) was compiled against a
// newer libc++ (LLVM 21) that hashes strings through an out-of-line
// std::__hash_memory. The NDK's libc++ still inlines that code, so the symbol
// is missing and the app fails to load. Provide it with the same hash the
// older headers use.
#include <cstddef>
#include <functional>

#if defined(_LIBCPP_VERSION) && _LIBCPP_VERSION < 210000
_LIBCPP_BEGIN_NAMESPACE_STD
__attribute__((visibility("hidden"))) size_t __hash_memory(const void* p, size_t n) noexcept {
  return __murmur2_or_cityhash<size_t>()(p, n);
}
_LIBCPP_END_NAMESPACE_STD
#endif
