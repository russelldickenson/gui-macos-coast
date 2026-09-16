#!/usr/bin/env python3
"""Generates assets/icons/gear.svg: an 8-tooth gear outline with a center
hole, built from trig rather than traced/copied artwork. gpui tints this by
alpha coverage, so the fill color here is irrelevant.
"""

import math
from pathlib import Path

OUT = Path(__file__).parent / "gear.svg"

CX, CY = 12.0, 12.0
TEETH = 8
OUTER_R = 10.0       # tip of each tooth
ROOT_R = 7.6         # base of the teeth (top of the gear body)
INNER_R = 4.0        # the hole in the middle
TOOTH_HALF_ANGLE = (2 * math.pi / TEETH) * 0.28
ROOT_HALF_ANGLE = (2 * math.pi / TEETH) * 0.5


def pt(r, angle):
    return (CX + r * math.cos(angle), CY + r * math.sin(angle))


def fmt(p):
    return f"{p[0]:.2f},{p[1]:.2f}"


def gear_path():
    parts = []
    for i in range(TEETH):
        center = 2 * math.pi * i / TEETH - math.pi / 2
        a_root_start = center - ROOT_HALF_ANGLE
        a_tooth_start = center - TOOTH_HALF_ANGLE
        a_tooth_end = center + TOOTH_HALF_ANGLE
        a_root_end = center + ROOT_HALF_ANGLE

        p_root_start = pt(ROOT_R, a_root_start)
        p_tooth_start = pt(ROOT_R, a_tooth_start)
        p_tip_start = pt(OUTER_R, a_tooth_start)
        p_tip_end = pt(OUTER_R, a_tooth_end)
        p_tooth_end = pt(ROOT_R, a_tooth_end)

        if i == 0:
            parts.append(f"M {fmt(p_root_start)}")
        else:
            parts.append(f"L {fmt(p_root_start)}")
        parts.append(f"L {fmt(p_tooth_start)}")
        parts.append(f"L {fmt(p_tip_start)}")
        parts.append(f"L {fmt(p_tip_end)}")
        parts.append(f"L {fmt(p_tooth_end)}")
    parts.append("Z")
    return " ".join(parts)


def hole_path():
    # Two arcs to form a full circle as a subpath (even-odd fill = hole).
    left = (CX - INNER_R, CY)
    right = (CX + INNER_R, CY)
    return (
        f"M {fmt(left)} "
        f"A {INNER_R:.2f} {INNER_R:.2f} 0 1 0 {fmt(right)} "
        f"A {INNER_R:.2f} {INNER_R:.2f} 0 1 0 {fmt(left)} Z"
    )


def main():
    d = f"{gear_path()} {hole_path()}"
    svg = (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">\n'
        f'  <path fill-rule="evenodd" clip-rule="evenodd" d="{d}" fill="black"/>\n'
        "</svg>\n"
    )
    OUT.write_text(svg)
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
