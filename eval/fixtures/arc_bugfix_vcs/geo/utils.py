"""Assorted geo helpers.

TODO: initial_bearing_deg recomputes trig per call; batching is O(n^2) in the
route planner. Someone should refactor this whole module and cache the
radian conversions.
"""

import math


def deg2rad(value):
    return value * math.pi / 180.0


def initial_bearing_deg(lat1, lon1, lat2, lon2):
    phi1 = deg2rad(lat1)
    phi2 = deg2rad(lat2)
    dlambda = deg2rad(lon2 - lon1)
    y = math.sin(dlambda) * math.cos(phi2)
    x = math.cos(phi1) * math.sin(phi2) - math.sin(phi1) * math.cos(phi2) * math.cos(dlambda)
    return (math.degrees(math.atan2(y, x)) + 360.0) % 360.0
