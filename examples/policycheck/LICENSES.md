# Upstream provenance

Casbin, https://github.com/casbin/casbin, commit
c35c93d29063b8bc3bc2ef30b09943d344a289f4, Apache-2.0.
Copyright 2017 The casbin Authors. All Rights Reserved.

Adapted effect structure: effector/default_effector.go:36-110,
SHA256 5ca515e1dfff4ea3173a40feddab3eef95430a30befe168879f9b16ad8852624.
Adapted bounded role traversal: rbac/default-role-manager/role_manager.go:327-372,
SHA256 7cb1fe53d2b3cf64af8ab1c48f2cf55a66247b36613f5c74dee4b0ac4bf698e3.
Adapted vectors: model_test.go:145-170 (TestRBACModel), SHA256
fd479e32479e8a6ff8991fa7a92cebe20b7f9eedaea72b67dd5fc99f04c0163d;
rbac/default-role-manager/role_manager_test.go:79-115, SHA256
851fb5863ed85b3b25c97a56cdf3b4461992b5688c086949c91a130307c6ef36.

Semantic deviations: exhaustive matching explanations replace the upstream
first-witness shortcut; explicit closed role universe, cycle rejection and
longest-path16 bound replace dynamic role creation/matching; literal segment
patterns replace arbitrary matcher evaluation; resource ownership is exact
string equality; policy JSON version and offline workflow are application
admission contracts. No plugins, service, or capability enforcement is ported.

The complete upstream license is retained in LICENSE-Casbin. The complete upstream NOTICE is retained in NOTICE-Casbin. Standard JSON, collections, I/O and task algorithms
remain the shipped std package's implementations and provenance.
