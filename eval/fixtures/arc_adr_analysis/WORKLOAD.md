# Workload

Typical traffic issues roughly 50,000 distinct keys per hour, but about 200
keys account for 90% of reads (a hot set). Keys are never explicitly
invalidated today, so the cache grows for the lifetime of the process.
