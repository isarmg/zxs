#include <stddef.h>
unsigned zxs_valgrind_running(void);
void zxs_valgrind_make_undefined(void *address, size_t length);
void zxs_valgrind_make_defined(void *address, size_t length);
