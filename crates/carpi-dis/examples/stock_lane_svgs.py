"""Stock-style lane arrow vectors for `small_icons` (24-unit grid).

The RNS-E's ROM arrows are square-cut stems, 45-degree mitred bends and broad
flat triangular heads. These draw each lane shape the same way, so the
grid-fitted 18 px lane arrows match the stock maneuver arrows. Writes SVGs
named like the Maps sources, so `small_icons` takes the directory as is:

    python stock_lane_svgs.py <out dir>
    cargo run -p carpi-dis --example small_icons -- <out dir> <tmp> 18 --rust 18 crates/carpi-dis/src/lane_icons_stock.rs
"""
import math
import os
import sys

STEM = 3.2   # stem width, units
HEAD = 10.0  # head base width
HLEN = 6.0   # head length


def head(tip, ang):
    """Triangle with its tip at `tip`, pointing along `ang` (degrees, 0 = up,
    clockwise)."""
    a = math.radians(ang)
    d = (math.sin(a), -math.cos(a))
    n = (d[1], -d[0])
    base = (tip[0] - d[0] * HLEN, tip[1] - d[1] * HLEN)
    p1 = (base[0] + n[0] * HEAD / 2, base[1] + n[1] * HEAD / 2)
    p2 = (base[0] - n[0] * HEAD / 2, base[1] - n[1] * HEAD / 2)
    return 'M%.2f,%.2f L%.2f,%.2f L%.2f,%.2f Z' % (tip + p1 + p2)


def back(tip, ang, k):
    a = math.radians(ang)
    return (tip[0] - math.sin(a) * k, tip[1] + math.cos(a) * k)


def svg(stroke_path, heads):
    return ('<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24">'
            '<path d="%s" fill="none" stroke="#000" stroke-width="%.2f" '
            'stroke-linejoin="miter" stroke-linecap="butt"/>'
            '<path d="%s" fill="#000"/></svg>') % (stroke_path, STEM, ' '.join(heads))


def shapes():
    x, y0 = 12.0, 23.0
    out = {}
    # straight
    tip = (x, 2.0)
    s = back(tip, 0, HLEN - 0.5)
    out['maneuver_straight.svg'] = svg('M%.1f,%.1f L%.2f,%.2f' % (x, y0, *s), [head(tip, 0)])
    for side, sign in (('right', 1), ('left', -1)):
        # slight: up, then 45 degrees
        tip = (x + sign * 8.0, 3.0)
        s = back(tip, 45 * sign, HLEN - 0.5)
        out['maneuver_turn_slight_%s.svg' % side] = svg(
            'M%.1f,%.1f L%.1f,13 L%.2f,%.2f' % (x, y0, x, *s), [head(tip, 45 * sign)])
        # normal: up, then square across
        tip = (x + sign * 11.0, 9.0)
        s = back(tip, 90 * sign, HLEN - 0.5)
        out['maneuver_turn_normal_%s.svg' % side] = svg(
            'M%.1f,%.1f L%.1f,9 L%.2f,%.2f' % (x, y0, x, *s), [head(tip, 90 * sign)])
        # sharp: up, then back down at 45 degrees
        tip = (x + sign * 9.0, 17.0)
        s = back(tip, 135 * sign, HLEN - 0.5)
        out['maneuver_turn_sharp_%s.svg' % side] = svg(
            'M%.1f,%.1f L%.1f,6 L%.2f,%.2f' % (x - sign * 2.0, y0, x - sign * 2.0, *s),
            [head(tip, 135 * sign)])
    # u-turns: up, square over the top, down
    for side, sign in (('left', -1), ('right', 1)):
        xs = x - sign * 3.5
        xr = xs + sign * 8.0
        tip = (xr, 20.0)
        s = back(tip, 180, HLEN - 0.5)
        out['maneuver_u_turn_%s.svg' % side] = svg(
            'M%.1f,%.1f L%.1f,4 L%.1f,4 L%.2f,%.2f' % (xs, y0, xs, xr, *s), [head(tip, 180)])
    return out


def main():
    d = sys.argv[1]
    os.makedirs(d, exist_ok=True)
    for name, body in shapes().items():
        with open(os.path.join(d, name), 'w') as f:
            f.write(body)
    print(d)


if __name__ == '__main__':
    main()
