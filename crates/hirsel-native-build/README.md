# hirsel-native-build

The native benchmark's build helper (`bench/native/build-hirsel`): it compiles one
benchmark source through the ordinary driver for each allocator and partition
configuration and prints each executable's compile, link and runtime-archive record
as JSON. It links the Buck2 runtime archives `HIRSEL_RUNTIME_ARCHIVES` names and is
built by `//:hirsel-native-build-bin-hirsel-native-build` on the release platform.
Its sources are sealed in `bench/native/manifest.json`.
