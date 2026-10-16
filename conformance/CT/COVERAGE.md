# Compile-time data evaluation and pending coverage

- **CT-1:** scalar and aggregate data constants, closed inline expressions, ordinary declared calls, arithmetic/conditional values, overflow/division/contract/fact failures and divergent evaluation have controls. Section 23 assigns CT-1 to evaluated failures and preserves static rule owners. Derive/body generation and the CRC example are cut from v1 (#5).
- **CT-2:** materialized and dependent constants have run witnesses. Closed frames reject outer runtime bindings and abstract type/size inputs while allowing internal bindings and ordinary calls. Resource, capability and callable graphs do not cross the v1 data-publication boundary.
- **CT-3:** reserved for future compile-time features. V1 requires no executable shape, Build or generated-body API; the existing syntax and keywords remain reserved.
- **CT-4:** self-cycle and mutual-cycle rejections retain accepted acyclic controls. Divergence tests CT-1 budget exhaustion, but numeric below/at/above-budget tests await #7's constant and charge model.
