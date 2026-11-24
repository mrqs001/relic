//! Full analytical polygons. Display simplification never changes membership.
use crate::error::{Error, Result};
use geo::{BoundingRect, Coord, Intersects, LineString, MultiPolygon, Polygon, Rect};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};

/// No vertex cap for accepted publisher geometry; callers pass a bound only
/// where it mirrors a build rule (OSM member lists).
pub const MAX_VERTICES: usize = usize::MAX;
#[derive(Clone)]
pub struct AreaGeometry {
    pub polygons: MultiPolygon<f64>,
    pub bounds: [f64; 4],
    pub vertices: usize,
}
fn invalid() -> Error {
    Error::invalid("Area geometry is malformed, topologically invalid or exceeds the vertex limit")
}
impl AreaGeometry {
    pub fn parse(value: &Value, max: usize, cancel: &AtomicBool) -> Result<Self> {
        Self::decode(value, max, cancel, true, false)
    }
    /// Immutable full geometries validated at reference activation.
    pub(crate) fn accepted(value: &Value) -> Result<Self> {
        Self::decode(value, MAX_VERTICES, &AtomicBool::new(false), false, false)
    }
    /// Some publisher marine coastlines have islands touching the enclosing
    /// coast at one vertex. Preserve those exact coordinates, with an acyclic
    /// contact graph; crossing edges, overlaps and disconnected interiors fail.
    pub fn parse_marine(value: &Value, max: usize, cancel: &AtomicBool) -> Result<Self> {
        Self::decode(value, max, cancel, true, true)
    }
    // SQL reads only geometry accepted by activate_boundaries. Rechecking topology
    // on every cold query would put import-sized work on interactive readers.
    fn decode(
        value: &Value,
        max: usize,
        cancel: &AtomicBool,
        validate: bool,
        contacts: bool,
    ) -> Result<Self> {
        let coordinates = value["coordinates"].as_array().ok_or_else(invalid)?;
        let groups: Vec<&Value> = match value["type"].as_str() {
            Some("Polygon") => vec![&value["coordinates"]],
            Some("MultiPolygon") => coordinates.iter().collect(),
            _ => return Err(invalid()),
        };
        if groups.is_empty() {
            return Err(invalid());
        }
        let mut vertices = 0;
        let mut polygons = vec![];
        for group in groups {
            let rings = group
                .as_array()
                .filter(|v| !v.is_empty())
                .ok_or_else(invalid)?;
            let mut parsed = vec![];
            let mut anchor: Option<f64> = None;
            for ring in rings {
                let points = ring
                    .as_array()
                    .filter(|v| v.len() >= 4)
                    .ok_or_else(invalid)?;
                vertices += points.len();
                if vertices > max {
                    return Err(invalid());
                }
                let mut line: Vec<Coord<f64>> = vec![];
                let mut first = None;
                for (index, p) in points.iter().enumerate() {
                    if index % 1024 == 0 && cancel.load(Ordering::Acquire) {
                        return Err(Error::cancelled());
                    }
                    let xy = p.as_array().filter(|p| p.len() == 2).ok_or_else(invalid)?;
                    let x = xy[0]
                        .as_f64()
                        .filter(|n| n.is_finite() && n.abs() <= 180.)
                        .ok_or_else(invalid)?;
                    let y = xy[1]
                        .as_f64()
                        .filter(|n| n.is_finite() && n.abs() <= 90.)
                        .ok_or_else(invalid)?;
                    if index == 0 {
                        first = Some([x, y]);
                    }
                    if index + 1 == points.len() && first != Some([x, y]) {
                        return Err(invalid());
                    }
                    let mut unwrapped = x;
                    if let Some(prev) = line.last() {
                        while unwrapped - prev.x > 180. {
                            unwrapped -= 360.;
                        }
                        while unwrapped - prev.x < -180. {
                            unwrapped += 360.;
                        }
                    }
                    line.push(Coord { x: unwrapped, y });
                }
                if line.first() != line.last() {
                    return Err(invalid());
                }
                let center = line.iter().map(|p| p.x).sum::<f64>() / line.len() as f64;
                if let Some(a) = anchor {
                    let shift = ((a - center) / 360.0_f64).round() * 360.;
                    for p in &mut line {
                        p.x += shift;
                    }
                } else {
                    anchor = Some(center);
                }
                parsed.push(LineString::new(line));
            }
            let exterior = parsed.remove(0);
            let polygon = Polygon::new(exterior, parsed);
            polygons.push(polygon);
        }
        let polygons = MultiPolygon(polygons);
        if validate {
            validate_topology(&polygons, cancel, contacts)?;
        }
        let rect = polygons.bounding_rect().ok_or_else(invalid)?;
        Ok(Self {
            bounds: [rect.min().x, rect.min().y, rect.max().x, rect.max().y],
            polygons,
            vertices,
        })
    }
    pub fn intersects(&self, query: [f64; 4]) -> bool {
        let [w, s, e, n] = query;
        let intervals = if w <= e {
            vec![(w, e)]
        } else {
            vec![(w, 180.), (-180., e)]
        };
        intervals.iter().any(|&(west, east)| {
            [-360., 0., 360.].iter().any(|shift| {
                self.polygons.intersects(&Rect::new(
                    Coord {
                        x: west + shift,
                        y: s,
                    },
                    Coord {
                        x: east + shift,
                        y: n,
                    },
                ))
            })
        })
    }
}
// Indexed segment checks avoid geo 0.32's quadratic ring validator. Touching
// hole boundaries are conservatively unsupported for administrative input; the
// marine path permits only isolated contacts with an acyclic contact graph.
fn validate_topology(
    shape: &MultiPolygon<f64>,
    cancel: &AtomicBool,
    allow_contacts: bool,
) -> Result<()> {
    use geo::{
        Contains, Point, Relate,
        coordinate_position::CoordPos,
        dimensions::Dimensions,
        line_intersection::{LineIntersection, line_intersection},
    };
    use rstar::{AABB, RTree, RTreeObject};
    #[derive(Clone)]
    struct Segment {
        line: geo::Line<f64>,
        index: usize,
        ring: usize,
        count: usize,
    }
    impl RTreeObject for Segment {
        type Envelope = AABB<[f64; 2]>;
        fn envelope(&self) -> Self::Envelope {
            let b = self.line.bounding_rect();
            AABB::from_corners([b.min().x, b.min().y], [b.max().x, b.max().y])
        }
    }
    let mut checks = 0usize;
    for polygon in &shape.0 {
        let rings = std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .collect::<Vec<_>>();
        let mut segments = vec![];
        for (ring, points) in rings.iter().enumerate() {
            for (index, line) in points.lines().enumerate() {
                if line.start == line.end {
                    return Err(invalid());
                }
                segments.push(Segment {
                    line,
                    index,
                    ring,
                    count: points.0.len() - 1,
                });
            }
        }
        let tree = RTree::bulk_load(segments.clone());
        let mut contacts = std::collections::BTreeMap::new();
        for a in &segments {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::cancelled());
            }
            for b in tree.locate_in_envelope_intersecting(&a.envelope()) {
                if (a.ring, a.index) >= (b.ring, b.index) {
                    continue;
                }
                checks += 1;
                if checks > 10_000_000 {
                    return Err(invalid());
                }
                if let Some(hit) = line_intersection(a.line, b.line) {
                    if allow_contacts
                        && a.ring != b.ring
                        && let LineIntersection::SinglePoint {
                            intersection,
                            is_proper: false,
                        } = hit
                    {
                        let key = (a.ring.min(b.ring), a.ring.max(b.ring));
                        if contacts
                            .insert(key, intersection)
                            .is_some_and(|old| old != intersection)
                        {
                            return Err(invalid());
                        }
                        continue;
                    }
                    let adjacent = a.ring == b.ring
                        && (a.index + 1 == b.index || (a.index == 0 && b.index + 1 == a.count));
                    if !adjacent
                        || !matches!(
                            hit,
                            LineIntersection::SinglePoint {
                                is_proper: false,
                                ..
                            }
                        )
                    {
                        return Err(Error::invalid(format!(
                            "Area rings intersect or retrace: ring {} segment {}, ring {} segment {}",
                            a.ring, a.index, b.ring, b.index
                        )));
                    }
                }
            }
        }
        // A ring-contact cycle (including two contacts for the same ring pair)
        // can cut the polygon interior into disconnected regions.
        let mut groups = (0..rings.len()).collect::<Vec<_>>();
        for &(a, b) in contacts.keys() {
            let ga = groups[a];
            let gb = groups[b];
            if ga == gb {
                return Err(invalid());
            }
            for g in &mut groups {
                if *g == gb {
                    *g = ga;
                }
            }
        }
        let shell = Polygon::new(polygon.exterior().clone(), vec![]);
        for (i, hole) in polygon.interiors().iter().enumerate() {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::cancelled());
            }
            let witness = hole
                .0
                .iter()
                .find(|p| !contacts.values().any(|c| c == *p))
                .ok_or_else(invalid)?;
            if !shell.contains(&Point(*witness)) {
                return Err(invalid());
            }
            let filled = Polygon::new(hole.clone(), vec![]);
            for other in polygon.interiors().iter().skip(i + 1) {
                let other_witness = other
                    .0
                    .iter()
                    .find(|p| !contacts.values().any(|c| c == *p))
                    .ok_or_else(invalid)?;
                if filled.contains(&Point(*other_witness))
                    || Polygon::new(other.clone(), vec![]).contains(&Point(*witness))
                {
                    return Err(invalid());
                }
            }
        }
    }
    for (i, a) in shape.0.iter().enumerate() {
        for b in shape.0.iter().skip(i + 1) {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::cancelled());
            }
            if !a
                .bounding_rect()
                .unwrap()
                .intersects(&b.bounding_rect().unwrap())
            {
                continue;
            }
            let relation = a.relate(b);
            if relation.get(CoordPos::Inside, CoordPos::Inside) == Dimensions::TwoDimensional
                || relation.get(CoordPos::OnBoundary, CoordPos::OnBoundary)
                    == Dimensions::OneDimensional
            {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod marine_contact_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn isolated_tangency_is_preserved_but_invalid_interiors_are_rejected() {
        let cancel = AtomicBool::new(false);
        let shell = json!([[0, 0], [10, 0], [10, 10], [0, 10], [0, 0]]);
        let polygon = |holes: Vec<Value>| {
            let mut rings = vec![shell.clone()];
            rings.extend(holes);
            json!({"type":"Polygon","coordinates":rings})
        };
        let tangent = polygon(vec![json!([[0, 5], [2, 4], [2, 6], [0, 5]])]);
        assert!(AreaGeometry::parse(&tangent, 100, &cancel).is_err());
        let area = AreaGeometry::parse_marine(&tangent, 100, &cancel).unwrap();
        assert!(!area.intersects([1.2, 4.9, 1.4, 5.1]));
        for shape in [
            polygon(vec![json!([[-1, 5], [2, 4], [2, 6], [-1, 5]])]), // crossing
            polygon(vec![json!([[0, 4], [2, 5], [0, 6], [0, 4]])]),   // shared edge
            polygon(vec![json!([[0, 5], [5, 4], [10, 5], [5, 6], [0, 5]])]), // disconnects
            polygon(vec![
                json!([[0, 5], [4, 2], [4, 8], [0, 5]]),
                json!([[0, 5], [2, 4], [2, 6], [0, 5]]),
            ]), // nested holes
            polygon(vec![
                json!([[0, 5], [2, 3], [5, 5], [2, 7], [0, 5]]),
                json!([[5, 5], [8, 3], [10, 5], [8, 7], [5, 5]]),
            ]), // contact cycle
        ] {
            assert!(AreaGeometry::parse_marine(&shape, 100, &cancel).is_err());
        }
        assert!(AreaGeometry::parse_marine(&tangent, 100, &AtomicBool::new(true)).is_err());
    }
}

/// Whether a query box can touch an outline whose bounding rectangle is
/// `bounds` (`[min x, min y, max x, max y]`), testing the same intervals and
/// ±360° shifts as [`AreaGeometry::intersects`]. Never false when the exact test
/// is true, because an outline lies inside its bounding rectangle; it lets the
/// SQL skip reading outlines that cannot intersect.
pub fn bounds_may_intersect(bounds: [f64; 4], [w, s, e, n]: [f64; 4]) -> bool {
    let [bx0, by0, bx1, by1] = bounds;
    if n < by0 || s > by1 {
        return false;
    }
    let intervals = if w <= e { vec![(w, e)] } else { vec![(w, 180.), (-180., e)] };
    intervals.iter().any(|&(west, east)| [-360., 0., 360.].iter().any(|shift| !(east + shift < bx0 || west + shift > bx1)))
}

/// Bounded shared cache for immutable geometry; at most one million vertices.
/// No simplified shape or centroid participates in this SQL predicate.
pub(crate) fn register(c: &rusqlite::Connection) -> Result<()> {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex, OnceLock},
    };
    type Cache = VecDeque<(String, Arc<AreaGeometry>)>;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    // A malformed bounds value is "may intersect": the exact test then decides.
    c.create_scalar_function(
        "relic_bounds_may_intersect",
        5,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let raw: Option<String> = ctx.get(0)?;
            let bounds = raw.and_then(|b| serde_json::from_str::<[f64; 4]>(&b).ok()).filter(|b| b.iter().all(|v| v.is_finite()) && b[0] <= b[2] && b[1] <= b[3]);
            Ok(match bounds {
                Some(b) => bounds_may_intersect(b, [ctx.get(1)?, ctx.get(2)?, ctx.get(3)?, ctx.get(4)?]),
                None => true,
            })
        },
    )?;
    c.create_scalar_function(
        "relic_area_intersects",
        5,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let raw: String = ctx.get(0)?;
            let key = crate::model::hash(raw.as_bytes());
            let cache = CACHE.get_or_init(|| Mutex::new(VecDeque::new()));
            let found = cache
                .lock()
                .map_err(|_| rusqlite::Error::InvalidQuery)?
                .iter()
                .find(|(k, _)| k == &key)
                .map(|(_, v)| v.clone());
            let shape = if let Some(v) = found {
                v
            } else {
                let value: Value = serde_json::from_str(&raw)
                    .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))?;
                let parsed = Arc::new(
                    AreaGeometry::decode(
                        &value,
                        MAX_VERTICES,
                        &AtomicBool::new(false),
                        false,
                        false,
                    )
                    .map_err(|e| {
                        rusqlite::Error::UserFunctionError(Box::new(std::io::Error::other(
                            e.message,
                        )))
                    })?,
                );
                let mut cache = cache.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
                while cache.iter().map(|(_, v)| v.vertices).sum::<usize>() + parsed.vertices
                    > 1_000_000
                    || cache.len() >= 128
                {
                    cache.pop_front();
                }
                cache.push_back((key, parsed.clone()));
                parsed
            };
            Ok(shape.intersects([ctx.get(1)?, ctx.get(2)?, ctx.get(3)?, ctx.get(4)?]))
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn parse(v: Value) -> AreaGeometry {
        AreaGeometry::parse(&v, 100, &AtomicBool::new(false)).unwrap()
    }
    #[test]
    fn the_bounds_check_never_rules_out_a_real_intersection() {
        let shapes = [
            json!({"type":"Polygon","coordinates":[[[34.2, 29.5], [35.9, 29.5], [35.9, 33.3], [34.2, 33.3], [34.2, 29.5]]]}),
            json!({"type":"Polygon","coordinates":[[[0, 0], [10, 0], [10, 10], [0, 10], [0, 0]], [[2, 2], [8, 2], [8, 8], [2, 8], [2, 2]]]}),
            // Touching the antimeridian from both sides.
            json!({"type":"Polygon","coordinates":[[[170, -20], [180, -20], [180, -10], [170, -10], [170, -20]]]}),
            json!({"type":"Polygon","coordinates":[[[-179.5, 60], [-170, 60], [-170, 70], [-179.5, 70], [-179.5, 60]]]}),
        ];
        // A small deterministic generator: boxes of every size, some crossing 180°.
        let mut x: u64 = 0x2545F4914F6CDD1D;
        let mut next = |lo: f64, hi: f64| { x ^= x << 13; x ^= x >> 7; x ^= x << 17; lo + (x % 1_000_000) as f64 / 1_000_000.0 * (hi - lo) };
        let mut exact_hits = 0;
        for shape in &shapes {
            let g = parse(shape.clone());
            for _ in 0..4000 {
                let (w, e) = (next(-180., 180.), next(-180., 180.));
                let (a, b) = (next(-85., 85.), next(-85., 85.));
                let query = [w, a.min(b), e, a.max(b)];
                if g.intersects(query) {
                    exact_hits += 1;
                    assert!(bounds_may_intersect(g.bounds, query), "{shape} {query:?}");
                }
            }
            // Boxes far from the outline are ruled out without reading it.
            assert!(!bounds_may_intersect(g.bounds, [g.bounds[2] + 1.0, -85., g.bounds[2] + 2.0, 85.]) || g.bounds[2] + 2.0 > 180.0);
        }
        assert!(exact_hits > 500, "the generator must exercise real intersections: {exact_hits}");
    }
    #[test]
    fn topology_rejects_nested_holes_spikes_and_overlapping_islands() {
        for coordinates in [
            json!([
                [[0, 0], [10, 0], [10, 10], [0, 10], [0, 0]],
                [[20, 20], [21, 20], [21, 21], [20, 20]]
            ]),
            json!([
                [[0, 0], [10, 0], [10, 10], [0, 10], [0, 0]],
                [[1, 1], [9, 1], [9, 9], [1, 9], [1, 1]],
                [[2, 2], [3, 2], [3, 3], [2, 2]]
            ]),
            json!([[[0, 0], [10, 0], [5, 0], [10, 10], [0, 10], [0, 0]]]),
            json!([
                [[0, 0], [10, 0], [10, 10], [0, 10], [0, 0]],
                [[0, 0], [1, 1], [1, 2], [0, 0]]
            ]),
        ] {
            assert!(
                AreaGeometry::parse(
                    &json!({"type":"Polygon","coordinates":coordinates}),
                    100,
                    &AtomicBool::new(false)
                )
                .is_err()
            );
        }
        assert!(AreaGeometry::parse(&json!({"type":"MultiPolygon","coordinates":[[[[0,0],[5,0],[5,5],[0,5],[0,0]]],[[[1,1],[2,1],[2,2],[1,2],[1,1]]]]}),100,&AtomicBool::new(false)).is_err());
    }
    #[test]
    fn holes_and_enclosing_viewports_use_full_geometry() {
        let p = parse(
            json!({"type":"Polygon","coordinates":[[[0,0],[10,0],[10,10],[0,10],[0,0]],[[3,3],[3,7],[7,7],[7,3],[3,3]]]}),
        );
        assert!(!p.intersects([4., 4., 6., 6.]));
        assert!(p.intersects([1., 1., 2., 2.]));
        assert!(p.intersects([-1., -1., 11., 11.]));
        assert!(p.intersects([9., 9., 12., 12.]));
        assert!(!p.intersects([11., 11., 12., 12.]));
    }
    #[test]
    fn crossing_dateline_does_not_fill_the_rest_of_the_world() {
        let p = parse(
            json!({"type":"Polygon","coordinates":[[[179,1],[-179,1],[-179,3],[179,3],[179,1]]]}),
        );
        assert!(p.intersects([178., 0., -178., 4.]));
        assert!(p.intersects([-180., 1., -179.5, 2.]));
        assert!(!p.intersects([-1., 0., 1., 4.]));
    }
    #[test]
    fn islands_and_invalid_polygons_remain_distinct() {
        let p = parse(
            json!({"type":"MultiPolygon","coordinates":[[[[0,0],[1,0],[1,1],[0,1],[0,0]]],[[[10,10],[11,10],[11,11],[10,11],[10,10]]]]}),
        );
        assert!(!p.intersects([4., 4., 6., 6.]));
        assert!(p.intersects([10.2, 10.2, 10.8, 10.8]));
        for v in [
            json!({"type":"Polygon","coordinates":[[[0,0],[10,10],[10,0],[0,10],[0,0]]]}),
            json!({"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,1]]]}),
            json!({"type":"Polygon","coordinates":[[[0,0],[181,0],[1,1],[0,0]]]}),
        ] {
            assert!(AreaGeometry::parse(&v, 100, &AtomicBool::new(false)).is_err());
        }
        assert!(
            AreaGeometry::parse(
                &json!({"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,0]]]}),
                3,
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert!(
            AreaGeometry::parse(
                &json!({"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,0]]]}),
                100,
                &AtomicBool::new(true)
            )
            .is_err()
        );
    }
}
