; ExclusiveAddress parameter attributes before optimization, on both targets.
; Hidden context and by-value private roots carry no address attributes.
declare i8 @empty(ptr, ptr noalias align 1)
declare i8 @byte(ptr, ptr noalias align 1 dereferenceable(1))
declare i8 @wide(ptr, ptr noalias align 16 dereferenceable(16))
declare i8 @aggregate(ptr, ptr noalias align 8 dereferenceable(24))
declare i8 @packed(ptr, ptr noalias align 8 dereferenceable(8))
declare i8 @root_list(ptr, ptr noalias align 8 dereferenceable(8))
declare i8 @root_box(ptr, ptr noalias align 8 dereferenceable(8))
