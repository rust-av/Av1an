#include <stdlib.h>

typedef void (*Deallocator)(void *);

/* Never called: handle lookup sees this decoy, ordinary relocations see the
 * process-global allocator instead. No symbolic binding or deep binding. */
void free(void *pointer) { (void)pointer; }

Deallocator cuda_fixture_bound_free(void) { return free; }
