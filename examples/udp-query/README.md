# UDP query

Run the deterministic echo peer without host network access:

```sh
hirsel run examples/udp-query/hirsel.json --grants examples/udp-query/grants.json
hirsel run --native examples/udp-query/hirsel.json --grants examples/udp-query/grants.json
```

Both print `sent 5; received 5 from port 19001; truncated false`.
`demo.json` supplies the same world to `--simulation`.
The program binds port 19000, sends one `hello` datagram to port 19001, receives
at most 512 bytes and closes the socket. The result exposes the peer and whether
the datagram exceeded the receive limit.

To use a host peer, change the template's network provider to `net.stream@1` and
remove `peers`. Its exact `udp_bind` and `udp_peer` rules remain necessary.
