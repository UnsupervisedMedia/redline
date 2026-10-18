// src/bezier.rs
use egui::{Pos2, Vec2};

use crate::types::BezierNode;

/// Subdivide a cubic Bezier segment between two nodes into `segments` polyline
/// points. Returns the (segments + 1) sample positions inclusive of endpoints.
pub fn cubic_samples(start: &BezierNode, end: &BezierNode, segments: usize) -> Vec<Pos2> {
    let p0 = start.pos;
    let p1 = start.pos + start.out_handle.unwrap_or(Vec2::ZERO);
    let p2 = end.pos + end.in_handle.unwrap_or(Vec2::ZERO);
    let p3 = end.pos;
    let n = segments.max(1);
    let mut pts = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let mt = 1.0 - t;
        let x = mt * mt * mt * p0.x
            + 3.0 * mt * mt * t * p1.x
            + 3.0 * mt * t * t * p2.x
            + t * t * t * p3.x;
        let y = mt * mt * mt * p0.y
            + 3.0 * mt * mt * t * p1.y
            + 3.0 * mt * t * t * p2.y
            + t * t * t * p3.y;
        pts.push(Pos2::new(x, y));
    }
    pts
}

/// Tessellate a full path of nodes. If `closed`, samples the last→first
/// segment too. Returns one continuous polyline.
pub fn tessellate(nodes: &[BezierNode], closed: bool, segments_per_curve: usize) -> Vec<Pos2> {
    if nodes.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    out.push(nodes[0].pos);
    for i in 0..nodes.len() - 1 {
        let samples = cubic_samples(&nodes[i], &nodes[i + 1], segments_per_curve);
        out.extend(samples.into_iter().skip(1));
    }
    if closed && nodes.len() > 1 {
        let samples = cubic_samples(nodes.last().unwrap(), &nodes[0], segments_per_curve);
        out.extend(samples.into_iter().skip(1));
    }
    out
}
