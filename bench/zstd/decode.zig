const std = @import("std");
extern "c" fn fopen([*:0]const u8, [*:0]const u8) ?*anyopaque;
extern "c" fn fseek(*anyopaque, c_long, c_int) c_int;
extern "c" fn ftell(*anyopaque) c_long;
extern "c" fn fread([*]u8, usize, usize, *anyopaque) usize;
extern "c" fn fclose(*anyopaque) c_int;
extern "c" fn printf([*:0]const u8, ...) c_int;
pub fn main(init: std.process.Init) !void {
    var args = init.minimal.args.iterate();
    _ = args.next();
    const name = args.next() orelse return error.Args;
    const f = fopen(name, "rb") orelse return error.Open;
    defer _ = fclose(f);
    if (fseek(f, 0, 2) != 0) return error.Seek;
    const n: usize = @intCast(ftell(f));
    if (fseek(f, 0, 0) != 0) return error.Seek;
    const data = try std.heap.page_allocator.alloc(u8, n);
    defer std.heap.page_allocator.free(data);
    if (fread(data.ptr, 1, n, f) != n) return error.Read;
    var sum: u64 = 0;
    for (0..10) |i| {
        var input: std.Io.Reader = .fixed(data);
        var output: std.Io.Writer.Allocating = .init(std.heap.page_allocator);
        defer output.deinit();
        var decoder: std.compress.zstd.Decompress = .init(&input, &.{}, .{});
        _ = try decoder.reader.streamRemaining(&output.writer);
        const decoded = output.written();
        if (i == 0) sum = std.hash.XxHash64.hash(0, decoded);
        sum +%= decoded.len + @as(u64, decoded[0]) + @as(u64, decoded[decoded.len - 1]);
    }
    _ = printf("%llu\n", sum);
}
