#include "valgrind-sdk.h"
#include <valgrind/memcheck.h>
unsigned zxs_valgrind_running(void) { return RUNNING_ON_VALGRIND; }
void zxs_valgrind_make_undefined(void *address, size_t length) {
    (void)VALGRIND_MAKE_MEM_UNDEFINED(address, length);
}
void zxs_valgrind_make_defined(void *address, size_t length) {
    (void)VALGRIND_MAKE_MEM_DEFINED(address, length);
}
