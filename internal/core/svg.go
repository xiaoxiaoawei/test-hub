package core

import (
	"fmt"
	"math"
	"regexp"
	"strings"
)

var pathTokens = regexp.MustCompile(`[A-Za-z]|[-+]?(?:\d*\.\d+|\d+\.?\d*)(?:[eE][-+]?\d+)?`)

// svgPath reads geometric SVG path data only. Curves are sampled into line segments.
func svgPath(s string) ([][]Point, error) {
	t := pathTokens.FindAllString(s, -1)
	if len(t) > 100000 {
		return nil, fmt.Errorf("SVG path exceeds size limit")
	}
	out := [][]Point{}
	pts := []Point{}
	cursor := Point{}
	start := Point{}
	cmd := ""
	i := 0
	push := func(p Point) { cursor = p; pts = append(pts, p) }
	flush := func() {
		if len(pts) > 0 {
			out = append(out, pts)
			pts = []Point{}
		}
	}
	for i < len(t) {
		if len(t[i]) == 1 && strings.Contains("MmLlHhVvCcQqAaZz", t[i]) {
			cmd = t[i]
			i++
			if cmd == "z" || cmd == "Z" {
				push(start)
				flush()
				cmd = ""
				continue
			}
		}
		if cmd == "" {
			return nil, fmt.Errorf("unsupported SVG command at %d", i)
		}
		upper := strings.ToUpper(cmd)
		count := map[string]int{"M": 2, "L": 2, "H": 1, "V": 1, "C": 6, "Q": 4, "A": 7}[upper]
		if i+count > len(t) {
			return nil, fmt.Errorf("truncated SVG %s", cmd)
		}
		v := make([]float64, count)
		for j := range v {
			if len(t[i+j]) == 1 && ((t[i+j][0] >= 'a' && t[i+j][0] <= 'z') || (t[i+j][0] >= 'A' && t[i+j][0] <= 'Z')) {
				return nil, fmt.Errorf("unsupported SVG command %s", t[i+j])
			}
			v[j] = num(t[i+j])
		}
		i += count
		relative := cmd == strings.ToLower(cmd)
		point := func(x, y float64) Point {
			if relative {
				return Point{x + cursor.X, y + cursor.Y}
			}
			return Point{x, y}
		}
		switch upper {
		case "M":
			p := point(v[0], v[1])
			flush()
			push(p)
			start = p
			if relative {
				cmd = "l"
			} else {
				cmd = "L"
			}
		case "L":
			push(point(v[0], v[1]))
		case "H":
			x := v[0]
			if relative {
				x += cursor.X
			}
			push(Point{x, cursor.Y})
		case "V":
			y := v[0]
			if relative {
				y += cursor.Y
			}
			push(Point{cursor.X, y})
		case "C":
			a, b, c, d := cursor, point(v[0], v[1]), point(v[2], v[3]), point(v[4], v[5])
			for j := 1; j <= 24; j++ {
				u := float64(j) / 24
				z := 1 - u
				push(Point{z*z*z*a.X + 3*z*z*u*b.X + 3*z*u*u*c.X + u*u*u*d.X, z*z*z*a.Y + 3*z*z*u*b.Y + 3*z*u*u*c.Y + u*u*u*d.Y})
			}
		case "Q":
			a, b, c := cursor, point(v[0], v[1]), point(v[2], v[3])
			for j := 1; j <= 24; j++ {
				u := float64(j) / 24
				z := 1 - u
				push(Point{z*z*a.X + 2*z*u*b.X + u*u*c.X, z*z*a.Y + 2*z*u*b.Y + u*u*c.Y})
			}
		case "A":
			end := point(v[5], v[6])
			for _, p := range arcPoints(cursor, end, v[0], v[1], v[2], v[3] != 0, v[4] != 0) {
				push(p)
			}
		}
	}
	flush()
	return out, nil
}

// SVG endpoint-to-center conversion from the SVG elliptical arc specification.
func arcPoints(a, b Point, rx, ry, angle float64, large, sweep bool) []Point {
	rx, ry = math.Abs(rx), math.Abs(ry)
	if rx == 0 || ry == 0 || math.Hypot(a.X-b.X, a.Y-b.Y) < 1e-9 {
		return []Point{b}
	}
	phi := angle * math.Pi / 180
	c, s := math.Cos(phi), math.Sin(phi)
	dx, dy := (a.X-b.X)/2, (a.Y-b.Y)/2
	xp, yp := c*dx+s*dy, -s*dx+c*dy
	lam := xp*xp/(rx*rx) + yp*yp/(ry*ry)
	if lam > 1 {
		rx *= math.Sqrt(lam)
		ry *= math.Sqrt(lam)
	}
	den := rx*rx*yp*yp + ry*ry*xp*xp
	k := 0.
	if den > 0 {
		k = math.Sqrt(math.Max(0, (rx*rx*ry*ry-den)/den))
	}
	if large == sweep {
		k = -k
	}
	cxp, cyp := k*rx*yp/ry, -k*ry*xp/rx
	cx, cy := c*cxp-s*cyp+(a.X+b.X)/2, s*cxp+c*cyp+(a.Y+b.Y)/2
	ux, uy := (xp-cxp)/rx, (yp-cyp)/ry
	vx, vy := (-xp-cxp)/rx, (-yp-cyp)/ry
	start := math.Atan2(uy, ux)
	delta := math.Atan2(ux*vy-uy*vx, ux*vx+uy*vy)
	if !sweep && delta > 0 {
		delta -= 2 * math.Pi
	}
	if sweep && delta < 0 {
		delta += 2 * math.Pi
	}
	segments := int(math.Ceil(math.Abs(delta) / (math.Pi / 72)))
	if segments < 2 {
		segments = 2
	}
	if segments > 288 {
		segments = 288
	}
	out := make([]Point, 0, segments)
	for j := 1; j <= segments; j++ {
		t := start + delta*float64(j)/float64(segments)
		out = append(out, Point{cx + c*rx*math.Cos(t) - s*ry*math.Sin(t), cy + s*rx*math.Cos(t) + c*ry*math.Sin(t)})
	}
	out[len(out)-1] = b
	return out
}
