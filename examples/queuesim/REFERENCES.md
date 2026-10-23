# Reference and semantic adaptations

SimPy, MIT, commit f43816490c6f76f336ad6e457d3cab9f386894af:

* src/simpy/core.py:165–213: heap ordering by time, priority and insertion ID;
  SHA256 05abddf46dca568598006fd42727f9b4913b47a8afb59359f999878a28fbf437.
* src/simpy/resources/resource.py:168–225: positive capacity, users and FIFO
  waiting requests; SHA256 780bcd474925ac74ea3e01c680a05f21c7232c43b9a973e1c6550e5d20d41a4f.
* tests/test_resources.py:17–43: two serial jobs complete at ticks 1 and 2;
  SHA256 466884e813e2c4f5e913ec32e313ac1221eae86e1c2232968b43d43f5030ca4f.
* tests/test_environment.py:10–53: empty schedule and exclusive horizon/resume;
  SHA256 f2ff5c38483155cca8321c6d74c32e10b30d7af817509d61dd3cf4b55e8176ff.

The MIT notice is retained in LICENSES. Generic typed Event payloads replace
Python generators/exceptions and callback lists. Dispatch transfers ownership
before state mutation. App-defined cancellation/release/arrival priorities make
simultaneous behavior explicit; they do not claim to reproduce every internal
SimPy Process/Request micro-event. Cancellations are non-preemptive. Inputs are
finite integer routes; no stochastic generator or feedback loop exists. The
standard priority queue provides the heap algorithm; no heap is duplicated.
A separate integer-tick model generates service-network observations.
