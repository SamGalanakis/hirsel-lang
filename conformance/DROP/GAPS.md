# Remaining destruction coverage

V20 ratifies unit-only, nonpropagating finalizers, bug failure during drop,
explicit @noalloc drop and structural noalloc_drop. New cleanup pairs cover generic
and concrete implicit destruction, allocating bodies and owned fields. Allocation
instrumentation must cover cancellation and failure exits as well as normal exits.

Cross-module forget and external resource destructuring need #4 package fixtures;
source declaration names cannot contain module paths. Same-module forget remains
covered. Cancellation-specific drop traces now have declared child-local Scope
and error signatures, but still need the M2 injected scheduler oracle. Unsafe-FFI
drop termination additionally needs #6 foreign metadata/harness support.
