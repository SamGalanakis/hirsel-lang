/* Hand-written C mirrors of the @layout(c) records in
 * crates/hirsel-lir/tests/layout.rs (`c_records`), and of the dataful variant
 * of its null-pointer niches (`niche_table_for`), which is the whole value:
 * `niche` names the pointer word whose null is the fieldless variant.
 * The layout test derives each and asserts clang's sizeof/alignof/offsetof
 * against it for x86-64 and AArch64 Linux. Written independently of the
 * derivation. */
#include <stddef.h>
#include <stdint.h>

struct CHeader {
    uint32_t f0;
    uint16_t f1;
    uint16_t f2;
};

struct CMixed {
    uint8_t f0;
    double f1;
    _Bool f2;
    int16_t f3;
    float f4;
};

struct CWide {
    uint8_t f0;
    __int128 f1;
    uint32_t f2;
};

struct CNested {
    uint8_t f0;
    struct CMixed f1;
    uint8_t f2[5];
    struct CMixed f3[3];
};

struct CPacked {
    uint8_t f0;
    uint64_t f1;
    uint16_t f2;
} __attribute__((packed));

struct CAligned {
    uint8_t f0;
    uint64_t f1;
} __attribute__((aligned(32)));

/* `Leaf | Node(Box[Tree], Box[Tree])`: the binarytrees node, C's two words. */
struct NicheTree {
    union {
        struct NicheTree *f0;
        struct NicheTree *niche;
    };
    struct NicheTree *f1;
};

/* `Node(i64, String, Box[f64]) | Leaf`: the niche word is the String's owner,
 * nested in the second field. */
struct NicheShifted {
    int64_t f0;
    union {
        struct {
            void *owner;
            uint64_t start;
            uint64_t length;
        } f1;
        void *niche;
    };
    double *f2;
};
