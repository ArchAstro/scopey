# Reproducible web experiments

## Methods

We implemented the service in Python using Flask. Requests were replayed against
the application test client, and response status and latency were recorded.
Flask was selected because its minimal routing API kept the experiment small.

## Software availability

The analysis repository contains the application and request fixtures. Python
package versions are not currently listed in the manuscript.
