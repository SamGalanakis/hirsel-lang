# hirsel-image

The std image's container format (design §1(f)): one file, mapped read-only,
holding a package's checked products so a program reads them without parsing
or checking the package. `hirsel_engine::image` (`StdImage`, `ImageRecord`)
decides what the sections hold; this crate knows only ids, tables and bytes.

Allowed dependencies: intern, span, syntax, types; bincode, serde and libc.

- **Canonical bytes.** Every session id is written as a number in a table the
  file carries: symbols and DefPath components in the string table, `DefId`s in
  the definition table (hirsel-intern `persist`, hirsel-syntax
  `with_def_numbering`), `TypeId`s as positions in the one type table
  (hirsel-types `TableBuilder`) and span anchors by (source manifest ordinal,
  base). `ImageWriter::finish` numbers keyed definitions first in DefPath
  order, then encodes sections in kind order and keyed entries in DefPath
  order, numbering each id as it meets it, then the type rows those reach. The
  bytes depend only on the products: not on id assignment (`id-salt`), the
  order the interner saw rows, or the order entries were added (`--jobs`).
- **Reading.** `Image::open` maps the file and decodes the type table eagerly
  into an immutable `TypeBase`: position p is base row p in every session
  interner over `Image::base()`, so a decoded `TypeId` is valid in all of them
  and one decoded entry serves every session. Strings and definitions resolve
  on first use; entries decode one at a time (`entry`, `decode`, `blob`), with
  bounded bincode limits, so a damaged file is refused rather than trusted.
  `Image::read_key` reads only the header, so a stale image is detected before
  it is mapped.
- **Replacement.** `write_atomically` writes a temporary file and renames it
  over the image, so an open mapping keeps reading the file it opened. Images
  are never rewritten in place.
- **Reduced entries.** An entry whose value does not encode (a span naming a
  source outside the image's anchors) is replaced by `Encode::reduced`, or
  dropped and reported in `Written::dropped`.

The layout is documented in `src/lib.rs`.
