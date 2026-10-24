// ELF entry point: LLVM 23 lld/unittests/AsLibELF/SomeDrivers.cpp.
// The upstream driver supplies argument parsing and crash recovery.
#include "lld/Common/Driver.h"
#include "llvm/Support/InitLLVM.h"

LLD_HAS_DRIVER(elf)

int main(int argc, char **argv) {
  llvm::InitLLVM initialization(argc, argv);
  const lld::DriverDef drivers[] = {{lld::Gnu, &lld::elf::link}};
  return lld::lldMain({argv, static_cast<size_t>(argc)}, llvm::outs(), llvm::errs(),
                      drivers)
      .retCode;
}
