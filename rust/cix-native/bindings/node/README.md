# CIX Node-API binding

This dependency-free C Node-API addon is an **installed CIX SDK consumer**. It requires N-API 8 because it uses object type tags to reject borrowed or forged receivers before native handles are interpreted. Configure CMake with absolute `CIX_SDK_INCLUDE_DIR`, `CIX_SDK_LIBRARY`, and `NODE_API_INCLUDE_DIR`; it never downloads packages, runs CIX, searches `PATH`, or builds the SDK.

`new Context(options)` exposes complete-buffer `encode(Buffer)` and `decode(Buffer)`. `new Encoder(options)` and `new Decoder(options)` expose the independent CIXG1 stream ABI: `process(buffer, capacity)`, `flush(capacity)`, `finish(capacity)`, `reset()`, `cancel()`, and `dispose()`. Each stream result is `{ data, consumed, produced, state }`. `dispose()` is idempotent and finalizers dispose abandoned native handles. Calls copy output into a Node Buffer; capacity, produced-total, input+output temporary buffers, and configured output/memory limits are checked before native entry.

`cancel()` is truthful cooperative cancellation: it blocks later addon calls, but the installed v1 C ABI has no native cancellation handle and cannot stop a call already executing. `reset()` clears this local cancellation fence. This addon does not claim full-engine routing, retained-history streaming, or a process-RSS hard limit.

`test/contract.js` is dependency-free and requires `CIX_NODE_ADDON` to name the explicitly built `.node` file; it never attempts a build.

## TypeScript package surface

`package.json` and `index.d.ts` describe the compiled `cix_node.node` add-on. The binding accepts **Node `Buffer`** inputs and JavaScript safe-integer `number` options/capacities; it does not accept generic `Uint8Array` inputs or `bigint` values. The package requires Node 18+ with N-API 8 and a compiled installed-SDK native add-on. It remains an SDK buffer/stream consumer and does not expose CIX full-engine capabilities.
