# Search fairness

Search-policy comparisons require identical corpus/tasks, initial anchors, structural objective, menu surface, algorithm version, beam/depth/child envelope, feature schema, compatible model version, successful runtime safety limits, compiler build, and dataset partition provenance.

Complete, bounded, cancelled, failed, terminal and no-terminal runs remain separate. Failures stay in the denominator. Task success, compiler rejection, ranking score, objective components, work, trajectory length and timing are reported separately; there is no opaque overall score.

After trajectory divergence, frame-level paired claims stop. Task-level comparison remains possible under the same objective/envelope with divergence stated explicitly. Test outcomes never enter training/model selection, and Stage 7A makes no general “learned search is better” claim.
