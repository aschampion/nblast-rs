use nalgebra::base::{Matrix3x5, Unit, Vector3};
use rstar::primitives::PointWithData;
use rstar::RTree;
use std::collections::HashMap;

// NOTE: will panic if this is changed due to use of Matrix3x5
const N_NEIGHBORS: usize = 5;

pub type Precision = f64;
type PointWithIndex = PointWithData<usize, [Precision; 3]>;

#[derive(Debug, Clone, Copy)]
pub struct DistDot {
    pub dist: Precision,
    pub dot: Precision,
}

impl Default for DistDot {
    fn default() -> Self {
        Self {
            dist: 0.0,
            dot: 1.0,
        }
    }
}

pub trait QueryNeuron {
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn query(
        &self,
        target: &impl TargetNeuron,
        score_fn: &impl Fn(&DistDot) -> Precision,
    ) -> Precision;

    fn self_hit(&self, score_fn: &impl Fn(&DistDot) -> Precision) -> Precision {
        score_fn(&DistDot::default()) * self.len() as Precision
    }

    fn points(&self) -> Vec<[Precision; 3]>;

    fn tangents(&self) -> Vec<Unit<Vector3<Precision>>>;
}

#[derive(Clone)]
pub struct QueryPointTangents {
    points: Vec<[Precision; 3]>,
    tangents: Vec<Unit<Vector3<Precision>>>,
}

fn subtract_points(p1: &[Precision; 3], p2: &[Precision; 3]) -> [Precision; 3] {
    let mut result = [0.0; 3];
    for ((rref, v1), v2) in result.iter_mut().zip(p1).zip(p2) {
        *rref = v1 - v2;
    }
    result
}

pub fn center_points<'a>(
    points: impl Iterator<Item = &'a [Precision; 3]>,
) -> impl Iterator<Item = [Precision; 3]> {
    let mut points_vec = Vec::default();
    let mut means: [Precision; 3] = [0.0, 0.0, 0.0];
    for pt in points {
        points_vec.push(*pt);
        for (sum, v) in means.iter_mut().zip(pt.iter()) {
            *sum += v;
        }
    }

    for val in means.iter_mut() {
        *val /= points_vec.len() as Precision;
    }
    let subtract = move |p| subtract_points(&p, &means);
    points_vec.into_iter().map(subtract)
}

pub fn points_to_tangent_eig<'a>(
    points: impl Iterator<Item = &'a [Precision; 3]>,
) -> Option<Unit<Vector3<Precision>>> {
    let cols_vec: Vec<Vector3<Precision>> = center_points(points)
        .map(|p| Vector3::from_column_slice(&p))
        .collect();
    let neighbor_mat = Matrix3x5::from_columns(&cols_vec);
    let inertia = neighbor_mat * neighbor_mat.transpose();
    let eig = inertia.symmetric_eigen();
    // TODO: new_unchecked
    // TODO: better copying in general
    Some(Unit::new_normalize(Vector3::from_iterator(
        eig.eigenvectors
            .column(eig.eigenvalues.argmax().0)
            .iter()
            .cloned(),
    )))
}

// ! doesn't work
// pub fn points_to_tangent_svd<'a>(
//     points: impl Iterator<Item = &'a [Precision; 3]>,
// ) -> Option<Unit<Vector3<Precision>>> {
//     let cols_vec: Vec<Vector3<Precision>> = center_points(points)
//         .map(|p| Vector3::from_column_slice(&p))
//         .collect();
//     let neighbor_mat = Matrix3x5::from_columns(&cols_vec);
//     let svd = neighbor_mat.svd(false, true);

//     let (idx, _val) = svd.singular_values.argmax();

//     svd.v_t.map(|v_t| {
//         Unit::new_normalize(Vector3::from_iterator(v_t.column(idx).iter().cloned()))
//     })
// }

fn points_to_rtree(points: &[[Precision; 3]]) -> Result<RTree<PointWithIndex>, &'static str> {
    if points.len() < N_NEIGHBORS {
        return Err("Not enough points");
    }

    Ok(RTree::bulk_load(
        points
            .iter()
            .enumerate()
            .map(|(idx, point)| PointWithIndex::new(idx, *point))
            .collect(),
    ))
}

fn points_to_rtree_tangents(
    points: &[[Precision; 3]],
) -> Result<(RTree<PointWithIndex>, Vec<Unit<Vector3<Precision>>>), &'static str> {
    let rtree = points_to_rtree(points)?;

    let mut tangents: Vec<Unit<Vector3<Precision>>> = Vec::with_capacity(rtree.size());

    for point in points.iter() {
        match points_to_tangent_eig(
            rtree
                .nearest_neighbor_iter(&point)
                .take(N_NEIGHBORS)
                .map(|pwd| pwd.position()),
        ) {
            Some(t) => tangents.push(t),
            None => return Err("Failed to SVD"),
        }
    }

    Ok((rtree, tangents))
}

impl QueryPointTangents {
    pub fn new(points: Vec<[Precision; 3]>) -> Result<Self, &'static str> {
        points_to_rtree_tangents(&points).map(|(_, tangents)| Self { points, tangents })
    }
}

impl QueryNeuron for QueryPointTangents {
    fn len(&self) -> usize {
        self.points.len()
    }

    fn query(
        &self,
        target: &impl TargetNeuron,
        score_fn: &impl Fn(&DistDot) -> Precision,
    ) -> Precision {
        let mut score_total: Precision = 0.0;
        for (q_pt, q_tan) in self.points.iter().zip(self.tangents.iter()) {
            score_total += score_fn(&target.nearest_match_dist_dot(q_pt, q_tan));
        }
        score_total
    }

    fn points(&self) -> Vec<[Precision; 3]> {
        self.points.clone()
    }

    fn tangents(&self) -> Vec<Unit<Vector3<Precision>>> {
        self.tangents.clone()
    }
}

pub trait TargetNeuron: QueryNeuron {
    /// For a given point and tangent vector,
    /// get the distance to its nearest neighbor and dot product with that neighbor's tangent
    fn nearest_match_dist_dot(
        &self,
        point: &[Precision; 3],
        tangent: &Unit<Vector3<Precision>>,
    ) -> DistDot;
}

#[derive(Clone)]
pub struct RStarPointTangents {
    rtree: RTree<PointWithIndex>,
    tangents: Vec<Unit<Vector3<Precision>>>,
}

impl RStarPointTangents {
    pub fn new(points: Vec<[Precision; 3]>) -> Result<Self, &'static str> {
        points_to_rtree_tangents(&points).map(|(rtree, tangents)| Self { rtree, tangents })
    }

    pub fn new_with_tangents(
        points: Vec<[Precision; 3]>,
        tangents: Vec<Unit<Vector3<Precision>>>,
    ) -> Result<Self, &'static str> {
        points_to_rtree(&points).map(|rtree| Self { rtree, tangents })
    }
}

impl QueryNeuron for RStarPointTangents {
    fn len(&self) -> usize {
        self.tangents.len()
    }

    fn query(
        &self,
        target: &impl TargetNeuron,
        score_fn: &impl Fn(&DistDot) -> Precision,
    ) -> Precision {
        let mut score_total: Precision = 0.0;
        for q_pt_idx in self.rtree.iter() {
            let dd = target.nearest_match_dist_dot(q_pt_idx.position(), &self.tangents[q_pt_idx.data]);
            let score = score_fn(&dd);
            score_total += score;
        }
        score_total
    }

    fn points(&self) -> Vec<[Precision; 3]> {
        let mut unsorted: Vec<&PointWithIndex> = self.rtree.iter().collect();
        unsorted.sort_by_key(|pwd| pwd.data);
        unsorted.into_iter().map(|pwd| *pwd.position()).collect()
    }

    fn tangents(&self) -> Vec<Unit<Vector3<Precision>>> {
        self.tangents.clone()
    }
}

impl TargetNeuron for RStarPointTangents {
    fn nearest_match_dist_dot(
        &self,
        point: &[Precision; 3],
        tangent: &Unit<Vector3<Precision>>,
    ) -> DistDot {
        self.rtree
            .nearest_neighbor_iter_with_distance(point)
            .next()
            .map(|(element, dist2)| {
                let this_tangent = self.tangents[element.data];
                let dot = this_tangent.dot(tangent).abs();
                DistDot { dist: dist2.sqrt(), dot }
            })
            .expect("impossible")
    }
}

// ? consider using nalgebra's Point3 in PointWithIndex, for consistency
// ^ can't implement rstar::Point for nalgebra::geometry::Point3 because of orphan rules
// TODO: replace Precision with float generic

/// Given the upper bounds of a number of bins, find which bin the value falls into.
/// Values outside of the range fall into the bottom or top bin.
fn find_bin_binary(value: Precision, upper_bounds: &[Precision]) -> usize {
    let raw = match upper_bounds.binary_search_by(|bound| bound.partial_cmp(&value).unwrap()) {
        Ok(v) => v + 1,
        Err(v) => v,
    };
    let highest = upper_bounds.len() - 1;
    if raw > highest {
        highest
    } else {
        raw
    }
}

// fn find_bin_linear(value: Precision, upper_bounds: &[Precision]) -> usize {
//     let mut out = 0;
//     for bound in upper_bounds.iter() {
//         if &value < bound {
//             return out;
//         }
//         out += 1;
//     }
//     out - 1
// }

/// Convert an empirically-derived table of NBLAST scores to a function
/// which can be passed to dotprop queries.
///
/// Cells are passed in dot-major order
/// i.e. if the original table had distance bins in the left margin
/// and dot product bins on the top margin,
/// the cells should be given in row-major order.
///
/// Each bin is identified by its upper bound:
/// the lower bound is implicitly the previous bin's upper bound, or zero.
/// The output is constrained to the limits of the table.
pub fn table_to_fn(
    dist_thresholds: Vec<Precision>,
    dot_thresholds: Vec<Precision>,
    cells: Vec<Precision>,
) -> impl Fn(&DistDot) -> Precision {
    if dist_thresholds.len() * dot_thresholds.len() != cells.len() {
        panic!("Number of cells in table do not match number of columns/rows");
    }

    move |dd: &DistDot| -> Precision {
        let col_idx = find_bin_binary(dd.dot, &dot_thresholds);
        let row_idx = find_bin_binary(dd.dist, &dist_thresholds);

        let lin_idx = row_idx * dot_thresholds.len() + col_idx;
        cells[lin_idx]
    }
}

#[derive(Clone)]
pub struct NblastArena<N, F>
where
    N: TargetNeuron,
    F: Fn(&DistDot) -> Precision,
{
    neurons_scores: Vec<(N, Precision)>,
    score_fn: F,
}

pub type NeuronIdx = usize;

// TODO: caching strategy
impl<N, F> NblastArena<N, F>
where
    N: TargetNeuron,
    F: Fn(&DistDot) -> Precision,
{
    pub fn new(score_fn: F) -> Self {
        Self {
            neurons_scores: Vec::default(),
            score_fn,
        }
    }

    fn next_id(&self) -> NeuronIdx {
        self.neurons_scores.len()
    }

    pub fn add_neuron(&mut self, neuron: N) -> NeuronIdx {
        let idx = self.next_id();
        let score = neuron.self_hit(&self.score_fn);
        self.neurons_scores.push((neuron, score));
        idx
    }

    pub fn query_target(
        &self,
        query_idx: NeuronIdx,
        target_idx: NeuronIdx,
        normalized: bool,
        symmetric: bool,
    ) -> Option<Precision> {
        // ? consider separate methods
        let q = self.neurons_scores.get(query_idx)?;
        let t = self.neurons_scores.get(target_idx)?;
        let mut score = q.0.query(&t.0, &self.score_fn);
        if normalized {
            score /= q.1;
        }
        if symmetric {
            let mut score2 = t.0.query(&q.0, &self.score_fn);
            if normalized {
                score2 /= t.1;
            }
            score = (score + score2) / 2.0;
        }
        Some(score)
    }

    pub fn queries_targets(
        &self,
        query_idxs: &[NeuronIdx],
        target_idxs: &[NeuronIdx],
        normalize: bool,
        symmetric: bool,
    ) -> HashMap<(NeuronIdx, NeuronIdx), Precision> {
        let mut out = HashMap::with_capacity(query_idxs.len() * target_idxs.len());

        for q_idx in query_idxs.iter() {
            for t_idx in target_idxs.iter() {
                let key = (*q_idx, *t_idx);
                if q_idx == t_idx {
                    if q_idx < &self.neurons_scores.len() {
                        let mut val = 1.0;
                        if !normalize {
                            val *= self
                                .neurons_scores
                                .get(*q_idx)
                                .expect("Already checked length")
                                .1;
                        }
                        out.insert(key, val);
                    }
                } else if symmetric {
                    match out.get(&(*t_idx, *q_idx)).map_or_else(
                        || self.query_target(*q_idx, *t_idx, normalize, true),
                        |s| Some(*s),
                    ) {
                        Some(s) => out.insert(key, s),
                        _ => None,
                    };
                } else {
                    match self.query_target(*q_idx, *t_idx, normalize, false) {
                        Some(s) => out.insert(key, s),
                        _ => None,
                    };
                }
            }
        }
        out
    }

    pub fn self_hit(&self, idx: NeuronIdx) -> Option<Precision> {
        self.neurons_scores.get(idx).map(|(_, s)| *s)
    }

    pub fn all_v_all(
        &self,
        normalize: bool,
        symmetric: bool,
    ) -> HashMap<(NeuronIdx, NeuronIdx), Precision> {
        let idxs: Vec<NeuronIdx> = (0..self.len()).collect();
        self.queries_targets(&idxs, &idxs, normalize, symmetric)
    }

    pub fn is_empty(&self) -> bool {
        self.neurons_scores.is_empty()
    }

    pub fn len(&self) -> usize {
        self.neurons_scores.len()
    }

    pub fn points(&self, idx: NeuronIdx) -> Option<Vec<[Precision; 3]>> {
        self.neurons_scores.get(idx).map(|(n, _)| n.points())
    }

    pub fn tangents(&self, idx: NeuronIdx) -> Option<Vec<Unit<Vector3<Precision>>>> {
        self.neurons_scores.get(idx).map(|(n, _)| n.tangents())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: Precision = 0.001;

    fn add_points(a: &[f64; 3], b: &[f64; 3]) -> [f64; 3] {
        let mut out = [0., 0., 0.];
        for (idx, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            out[idx] = x + y;
        }
        out
    }

    fn make_points(offset: &[f64; 3], step: &[f64; 3], count: usize) -> Vec<[f64; 3]> {
        let mut out = Vec::default();
        out.push(*offset);

        for _ in 0..count - 1 {
            let to_push = add_points(out.last().unwrap(), step);
            out.push(to_push);
        }

        out
    }

    #[test]
    fn construct() {
        let points = make_points(&[0., 0., 0.], &[1., 0., 0.], 10);
        QueryPointTangents::new(points.clone()).expect("Query construction failed");
        RStarPointTangents::new(points).expect("Target construction failed");
    }

    fn is_close(val1: Precision, val2: Precision) -> bool {
        (val1 - val2).abs() < EPSILON
    }

    fn assert_close(val1: Precision, val2: Precision) {
        if !is_close(val1, val2) {
            panic!("Not close:\n\t{:?}\n\t{:?}", val1, val2);
        }
    }

    // #[test]
    // fn unit_tangents_svd() {
    //     let (points, _) = tangent_data();
    //     let tangent = points_to_tangent_svd(points.iter()).expect("SVD failed");
    //     assert_close(tangent.dot(&tangent), 1.0)
    // }

    #[test]
    fn unit_tangents_eig() {
        let (points, _) = tangent_data();
        let tangent = points_to_tangent_eig(points.iter()).expect("eig failed");
        assert_close(tangent.dot(&tangent), 1.0)
    }

    fn equivalent_tangents(
        tan1: &Unit<Vector3<Precision>>,
        tan2: &Unit<Vector3<Precision>>,
    ) -> bool {
        is_close(tan1.dot(tan2).abs(), 1.0)
    }

    fn tangent_data() -> (Vec<[Precision; 3]>, Unit<Vector3<Precision>>) {
        // calculated from implementation known to be correct
        let expected = Unit::new_normalize(
            Vector3::from_column_slice(&[-0.939_392_2 ,  0.313_061_82,  0.139_766_18])
        );

        // points in first row of data/dotprops/ChaMARCM-F000586_seg002.csv
        let points = vec![
            [329.679_962_158_203, 72.718_803_405_761_7, 31.028_469_085_693_4],
            [328.647_399_902_344, 73.046_119_689_941_4, 31.537_061_691_284_2],
            [335.219_879_150_391, 70.710_479_736_328_1, 30.398_145_675_659_2],
            [332.611_389_160_156, 72.322_929_382_324_2, 30.887_334_823_608_4],
            [331.770_782_470_703, 72.434_440_612_793, 31.169_372_558_593_8],
        ];

        (points, expected)
    }

    // #[test]
    // #[ignore]
    // fn test_tangent_svd() {
    //     let (points, expected) = tangent_data();
    //     let tangent = points_to_tangent_svd(points.iter()).expect("Failed to create tangent");
    //     println!("tangent is {:?}", tangent);
    //     println!("  expected {:?}", expected);
    //     assert!(equivalent_tangents(&tangent, &expected))
    // }

    #[test]
    fn test_tangent_eig() {
        let (points, expected) = tangent_data();
        let tangent = points_to_tangent_eig(points.iter()).expect("Failed to create tangent");
        if !equivalent_tangents(&tangent, &expected) {
            panic!("Non-equivalent tangents:\n\t{:?}\n\t{:?}", tangent, expected)
        }
    }

    /// dist_thresholds, dot_thresholds, values
    fn score_mat() -> (Vec<Precision>, Vec<Precision>, Vec<Precision>) {
        let dists = vec![10.0, 20.0, 30.0, 40.0, 50.0];
        let dots = vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0];
        let mut values = vec![];
        let n_values = dots.len() * dists.len();
        for v in 0..n_values {
            values.push(v as Precision);
        }
        (dists, dots, values)
    }

    #[test]
    fn test_score_fn() {
        let (dists, dots, values) = score_mat();
        let func = table_to_fn(dists, dots, values);
        assert_close(func(&DistDot{dist: 0.0, dot: 0.0}), 0.0);
        assert_close(func(&DistDot{dist: 0.0, dot: 0.1}), 1.0);
        assert_close(func(&DistDot{dist: 11.0, dot: 0.0}), 10.0);
        assert_close(func(&DistDot{dist: 55.0, dot: 0.0}), 40.0);
        assert_close(func(&DistDot{dist: 55.0, dot: 10.0}), 49.0);
        assert_close(func(&DistDot{dist: 15.0, dot: 0.15}), 11.0);
    }

    // #[test]
    // fn test_find_bin_linear() {
    //     let dots = vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0];
    //     assert_eq!(find_bin_linear(0.0, &dots), 0);
    //     assert_eq!(find_bin_linear(0.15, &dots), 1);
    //     assert_eq!(find_bin_linear(0.95, &dots), 9);
    //     assert_eq!(find_bin_linear(-10.0, &dots), 0);
    //     assert_eq!(find_bin_linear(10.0, &dots), 9);
    //     assert_eq!(find_bin_linear(0.1, &dots), 1);
    // }

    #[test]
    fn test_find_bin_binary() {
        let dots = vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0];
        assert_eq!(find_bin_binary(0.0, &dots), 0);
        assert_eq!(find_bin_binary(0.15, &dots), 1);
        assert_eq!(find_bin_binary(0.95, &dots), 9);
        assert_eq!(find_bin_binary(-10.0, &dots), 0);
        assert_eq!(find_bin_binary(10.0, &dots), 9);
        assert_eq!(find_bin_binary(0.1, &dots), 1);
    }

    #[test]
    fn score_function() {
        let dist_thresholds = vec![1.0, 2.0];
        let dot_thresholds = vec![0.5, 1.0];
        let cells = vec![1.0, 2.0, 4.0, 8.0];

        let score_fn = table_to_fn(dist_thresholds, dot_thresholds, cells);

        let q_points = make_points(&[0., 0., 0.], &[1.0, 0.0, 0.0], 10);
        let query = QueryPointTangents::new(q_points.clone()).expect("Query construction failed");
        let query2 = RStarPointTangents::new(q_points).expect("Construction failed");
        let target = RStarPointTangents::new(make_points(&[0.5, 0., 0.], &[1.1, 0., 0.], 10))
            .expect("Construction failed");

        assert_close(
            query.query(&target, &score_fn),
            query2.query(&target, &score_fn),
        );
        assert_close(query.self_hit(&score_fn), query2.self_hit(&score_fn));
        let score = query.query(&query2, &score_fn);
        let self_hit = query.self_hit(&score_fn);
        println!("score: {:?}, self-hit {:?}", score, self_hit);
        assert_close(query.query(&query2, &score_fn), query.self_hit(&score_fn));
    }

    #[test]
    fn arena() {
        let dist_thresholds = vec![1.0, 2.0];
        let dot_thresholds = vec![0.5, 1.0];
        let cells = vec![1.0, 2.0, 4.0, 8.0];

        let score_fn = table_to_fn(dist_thresholds, dot_thresholds, cells);

        let query = RStarPointTangents::new(make_points(&[0., 0., 0.], &[1., 0., 0.], 10))
            .expect("Construction failed");
        let target = RStarPointTangents::new(make_points(&[0.5, 0., 0.], &[1.1, 0., 0.], 10))
            .expect("Construction failed");

        let mut arena = NblastArena::new(score_fn);
        let q_idx = arena.add_neuron(query);
        let t_idx = arena.add_neuron(target);

        let no_norm = arena
            .query_target(q_idx, t_idx, false, false)
            .expect("should exist");
        let self_hit = arena
            .query_target(q_idx, q_idx, false, false)
            .expect("should exist");

        assert!(
            arena
                .query_target(q_idx, t_idx, true, false)
                .expect("should exist")
                - no_norm / self_hit
                < EPSILON
        );
        assert_eq!(
            arena.query_target(q_idx, t_idx, false, true),
            arena.query_target(t_idx, q_idx, false, true),
        );

        let out = arena.queries_targets(&[q_idx, t_idx], &[t_idx, q_idx], false, false);
        assert_eq!(out.len(), 4);
    }
}
