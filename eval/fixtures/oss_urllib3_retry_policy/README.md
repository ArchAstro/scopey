# Retry policy fixture

Transient failures receive one immediate retry. Later consecutive failures use
exponential backoff. Consumers depend on the first retry remaining latency-free.
