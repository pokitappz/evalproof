# Statistical gate specification

The outcome measured is a grader error relative to the reviewed contract. For the incorrect-output population, accepting an output is an error. For valid controls, rejecting an output is an error.

The engine freezes the generated cases and creates two uniform populations. It samples with replacement. Every critical mutation or control adds a separately evaluated metric. Sampling is reproducible from the saved seed, metric ID and sample index. Repeated samples require fresh, independent grader invocations; an SDK response cache is incompatible with this assumption.

## Confidence and stopping

Let `G` be the number of metrics and `alpha = 1 - confidence`. The six predeclared sample counts are 20, 40, 80, 160, 320 and 640 per metric. Each decision uses a two-sided Clopper-Pearson interval with interval error allowance `alpha / (6 * G)`, divided equally between its tails.

By the union bound, all intervals at all scheduled looks simultaneously cover their metric's true error probability with probability at least `1 - alpha`. Decisions may stop early at those looks without invalidating that bound. The count and sampling populations cannot change after observations begin.

- PASS when every upper bound is at or below the tolerance.
- FAIL when any lower bound exceeds the tolerance.
- Otherwise INCONCLUSIVE until another predeclared look is reached or execution stops.

At 5% tolerance and 99% run confidence, two clean metrics pass at 160 calls each. Four metrics require the 320-call look even with no errors. Near-boundary graders can remain inconclusive after 640 samples. This is expected behavior.

The guarantee is conditional on independent Bernoulli observations with a stable probability for each frozen population. A provider changing models mid-run, correlated sessions, hidden response caching or time-dependent grading can invalidate the model. Pin the model where possible and identify its configuration. The result does not claim production-wide accuracy, worst-case correctness of every unmarked fixture, or simultaneous confidence across an unlimited sequence of new runs.

## Resume and failure behavior

Reservations and in-flight counts are saved before dispatch. Completed observations are saved after every batch. Budget increases resume the existing seed, observations, metrics and look schedule. Cache reuse never adds samples. An interrupted in-flight request has an unknown outcome and possibly incurred charges; the run becomes ERROR rather than silently retrying and discarding evidence.

Evidence older than 24 hours cannot be resumed as a current run. A new explicitly identified run is needed after a terminal execution error or exhausted sampling plan. Repeatedly creating new runs until one passes undermines the intended workflow and is not covered by the per-run confidence claim.

## Verification and release status

`scripts/verify_statistics.py` checks Rust beta-quantile bounds against a separate implementation using direct binomial probability sums and bisection. It also checks exact false-decision tail bounds across multiple probabilities and metric counts. Unit and integration tests cover critical metrics, baseline false rejections, partial budgets, repeated runs and interrupted reservations.

Independent human statistical review and real-provider operational validation remain release prerequisites. The included mock-grader tests establish implementation behavior; they do not establish independence or stability of an external LLM service.

References: [NIST exact binomial confidence limits](https://www.itl.nist.gov/div898/software/dataplot/refman2/auxillar/exacbici.htm), [statrs 0.19.1](https://docs.rs/statrs/0.19.1/statrs/distribution/struct.Beta.html).
