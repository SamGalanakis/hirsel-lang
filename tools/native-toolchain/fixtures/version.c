/* Prints the version of the libLLVM the loader resolved, as major.minor.patch. */
#include <llvm-c/Core.h>
#include <stdio.h>

int main(void) {
  unsigned major = 0, minor = 0, patch = 0;
  LLVMGetVersion(&major, &minor, &patch);
  printf("%u.%u.%u\n", major, minor, patch);
  return 0;
}
