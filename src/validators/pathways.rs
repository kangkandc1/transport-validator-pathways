use crate::{Issue, IssueType, Severity};
use gtfs_structures::{Availability, Gtfs, Id, LocationType, Pathway, PathwayDirectionType, PathwayMode, RawGtfs, Stop, StopAccess};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;
use clap::builder::Str;
use rayon::prelude::*;
use geo::{Coord, Distance as _, Haversine, Point};
use itertools::Itertools;
use crate::custom_rules::{custom_rules, CustomRules};
use serde::ser::{SerializeStruct, Serializer};
use serde::Serialize;
use crate::IssueType::PathwaysImpliesTransitiveInconsistency;

#[derive(Debug)]
pub struct  PathwayIssue{
    base_issue: Issue,
    pathway_id: Option<String>,
    involved_stop_ids: Vec<String>,
}

impl  PathwayIssue {

    pub fn new(base_issue: Issue, pathway_id: Option<String>,involved_stop_ids:Vec<String>) -> Self {
        PathwayIssue{base_issue, pathway_id, involved_stop_ids}
    }

    pub fn new_stop_issue(base_issue: Issue, involved_stop_ids: Vec<String>) -> Self {
        PathwayIssue{base_issue, pathway_id: None, involved_stop_ids}
    }

}

impl Serialize for PathwayIssue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("PathwayIssue", 4)?;
        state.serialize_field("issue_type", &self.base_issue.issue_type)?;
        state.serialize_field("severity", &self.base_issue.severity)?;
        state.serialize_field("pathway_id", &self.pathway_id)?;
        state.serialize_field("involved_stop_ids", &self.involved_stop_ids)?;

        state.end()
    }
}

pub struct IntransitiveTriad{
    station_id: String,
    stop_a_id : String,
    stop_b_id : String,
    stop_c_id : String,
    pathway_1_id : String,
    pathway_2_id : String,
}

impl IntransitiveTriad {
    pub fn new(station_id:String,stop_a_id:String,stop_b_id:String,stop_c_id:String,pathway_1_id:String,pathway_2_id:String) -> Self {
        IntransitiveTriad{station_id,stop_a_id,stop_b_id,stop_c_id,pathway_1_id,pathway_2_id}
    }
}
pub struct StationWithPathwayIssue {
    station_id: String,
    coordinates : Option<Coord>,
    children: Vec<Arc<Stop>>,
    edges: Vec<Arc<Pathway>>,
    pathway_issues: Vec<PathwayIssue>,
    foreign_stops: Vec<Arc<Stop>>,
}

impl Serialize for StationWithPathwayIssue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Station", 6)?;
        state.serialize_field("station_id", &self.station_id)?;
        if let Some(ref coord) = self.coordinates {
            state.serialize_field("coordinates", &[coord.x, coord.y])?;
        }
        state.serialize_field("children", &self.children)?;
        state.serialize_field("edges", &self.edges)?;
        state.serialize_field("pathway_issues", &self.pathway_issues)?;
        state.serialize_field("foreign_stops", &self.foreign_stops)?;
        state.end()
    }
}

impl StationWithPathwayIssue {
    pub fn new(station_id: String, coordinates: Option<Coord>, children: Vec<Arc<Stop>>,edges: Vec<Arc<Pathway>>) -> Self {
        StationWithPathwayIssue {station_id, coordinates, children, edges, pathway_issues: vec![], foreign_stops: vec![] }
    }

    pub fn add_issue(&mut self, issue: PathwayIssue) {
        self.pathway_issues.push(issue);
    }

    pub fn add_foreign_stop(&mut self, stop: Arc<Stop>) {
        // Avoid duplicates
        if !self.foreign_stops.iter().any(|s| s.id == stop.id) && !self.children.iter().any(|c| c.id == stop.id) {
            self.foreign_stops.push(stop);
        }
    }
}
pub struct PathwayDirectedGraph{
    adjacent_nodes: HashMap<String,Vec<String>>,
    adjacent_nodes_wheelchair_accessible : HashMap<String,Vec<String>>,
    nodes_reachable_by_walkway : HashMap<String,HashSet<String>>,
    nodes_reachable_by_elevator_or_stairs : HashMap<String,HashSet<String>>,
    max_slope_ramp: f64,
    require_walkway_slope : bool
}

impl PathwayDirectedGraph {

    pub fn new(max_slope_ramp: f64, require_walkway_slope: bool) -> Self {
        PathwayDirectedGraph{adjacent_nodes: HashMap::new(), adjacent_nodes_wheelchair_accessible: Default::default(), nodes_reachable_by_walkway: Default::default(), nodes_reachable_by_elevator_or_stairs: Default::default(), max_slope_ramp, require_walkway_slope }
    }
    pub fn add_stop(&mut self, stop: &Stop) {
        // Ensure the stop exists as a node even if it has no pathways.
        let outgoing = self.adjacent_nodes.entry(stop.id.clone()).or_default();
        let outgoing_stepless = self.adjacent_nodes_wheelchair_accessible.entry(stop.id.clone()).or_default();
        let walkable_without_assitannce = self.nodes_reachable_by_walkway.entry(stop.id.clone()).or_default();
        outgoing.extend(stop.pathways.iter().map(|p| p.to_stop_id.clone()));
        let max_slope = self.max_slope_ramp;
        let require_walkway_slope = self.require_walkway_slope;
        outgoing_stepless.extend(stop.pathways.iter().filter(|p| PathwayDirectedGraph::is_pathway_accessible_by_wheelchair_static(p, max_slope, require_walkway_slope)).map(|p| p.to_stop_id.clone()));
        walkable_without_assitannce.extend(stop.pathways.iter().filter(|p| Self::is_pathway_walkable_without_assistance(p)).map(|p| p.to_stop_id.clone()));
        let elev_or_stairs = self.nodes_reachable_by_elevator_or_stairs.entry(stop.id.clone()).or_default();
        elev_or_stairs.extend(stop.pathways.iter().filter(|p| Self::is_pathway_elevator_or_stairs(p)).map(|p| p.to_stop_id.clone()));

        // Add the reverse edge for bidirectional pathways.
        for pathway in &stop.pathways {
            if matches!(pathway.is_bidirectional, PathwayDirectionType::Bidirectional) {
                self.adjacent_nodes
                    .entry(pathway.to_stop_id.clone())
                    .or_default()
                    .push(stop.id.clone());

                if self.is_pathway_accessible_by_wheelchair(&pathway, self.max_slope_ramp) {
                    self.adjacent_nodes_wheelchair_accessible
                        .entry(pathway.to_stop_id.clone())
                        .or_default()
                        .push(stop.id.clone());
                }

                if Self::is_pathway_walkable_without_assistance(&pathway) {
                    self.nodes_reachable_by_walkway.entry(pathway.to_stop_id.clone())
                    .or_default()
                        .insert(stop.id.clone());
                }

                if Self::is_pathway_elevator_or_stairs(&pathway) {
                    self.nodes_reachable_by_elevator_or_stairs.entry(pathway.to_stop_id.clone())
                    .or_default()
                        .insert(stop.id.clone());
                }
            }
        }
    }



    pub fn is_reachable(&self, from: &str, to: &str) -> bool {
        // A node is trivially reachable from itself, provided it exists.
        // Note: `to` is deliberately not checked for membership, because
        // targets of unidirectional pathways may not be keys yet.
        if from == to {
            return self.adjacent_nodes.contains_key(from);
        }

        let mut visited: HashSet<&str> = HashSet::from([from]);
        let mut stack: Vec<&str> = vec![from];

        while let Some(current) = stack.pop() {
            let Some(neighbors) = self.adjacent_nodes.get(current) else {
                continue; // dangling node with no outgoing edges
            };

            for neighbor in neighbors {
                let neighbor = neighbor.as_str();
                if neighbor == to {
                    return true;
                }
                if visited.insert(neighbor) {
                    stack.push(neighbor);
                }
            }
        }

        false
    }

    pub fn is_reachable_by_wheelchair(&self, from: &str, to: &str) -> bool {
        // A node is trivially reachable from itself, provided it exists.
        // Note: `to` is deliberately not checked for membership, because
        // targets of unidirectional pathways may not be keys yet.
        if from == to {
            return self.adjacent_nodes_wheelchair_accessible.contains_key(from);
        }

        let mut visited: HashSet<&str> = HashSet::from([from]);
        let mut stack: Vec<&str> = vec![from];

        while let Some(current) = stack.pop() {
            let Some(neighbors) = self.adjacent_nodes_wheelchair_accessible.get(current) else {
                continue; // dangling node with no outgoing edges
            };

            for neighbor in neighbors {
                let neighbor = neighbor.as_str();
                if neighbor == to {
                    return true;
                }
                if visited.insert(neighbor) {
                    stack.push(neighbor);
                }
            }
        }

        false
    }

    fn is_pathway_accessible_by_wheelchair_static(pathway: &Pathway, max_slope_ramp: f64, require_walkway_slope: bool) -> bool {
        if require_walkway_slope {
            return match pathway.mode{
                PathwayMode::Elevator => true,
                PathwayMode::Walkway | PathwayMode::MovingSidewalk => pathway
                    .max_slope
                    .is_some_and(|slope| slope.abs() <= max_slope_ramp as f32),
                _ => false,
            }
        }
        match pathway.mode{
            PathwayMode::Elevator => true,
            PathwayMode::Walkway | PathwayMode::MovingSidewalk => pathway
                .max_slope
                .is_none_or(|slope| slope.abs() <= max_slope_ramp as f32),
            _ => false,
            }
    }

    fn is_pathway_accessible_by_wheelchair(&self, pathway: &Pathway, max_slope_ramp: f64) -> bool {
        Self::is_pathway_accessible_by_wheelchair_static(pathway, max_slope_ramp, self.require_walkway_slope)
    }

    fn is_pathway_walkable_without_assistance(pathway: &Pathway) -> bool {
        matches ! (
            pathway.mode,
            PathwayMode::Walkway| PathwayMode::MovingSidewalk
        )
    }

    fn is_pathway_elevator_or_stairs(pathway: &Pathway) -> bool {
        matches ! (
            pathway.mode,
            PathwayMode::Elevator| PathwayMode::Stairs| PathwayMode::Escalator
        )
    }

}

impl fmt::Display for PathwayDirectedGraph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut keys: Vec<&String> = self.adjacent_nodes.keys().collect();
        let mut keys_stepless: Vec<&String> = self.adjacent_nodes_wheelchair_accessible.keys().collect();
        keys.sort();
        keys_stepless.sort();

        for key in keys {
            let neighbours = &self.adjacent_nodes[key];
            let neighbours_stepless = &self.adjacent_nodes_wheelchair_accessible[key];
            if neighbours.is_empty() {
                writeln!(f, "{key} (no outgoing pathways)")?;
            } else {
                writeln!(f, "{key} -> {}", neighbours.join(", "))?;
            }
            if neighbours_stepless.is_empty() {
                writeln!(f, "{key} (no stepless outgoing pathways)")?;
            } else {
                writeln!(f, "{key} stepless -> {}", neighbours_stepless.join(", "))?;
            }
        }
        Ok(())
    }
}

pub fn validate(gtfs: &gtfs_structures::Gtfs,custom_rules: &CustomRules) -> Vec<Issue> {
    let pathway_with_invalid_references_issue: Vec<Issue> = get_pathways_with_invalid_stop_references(gtfs).into_iter()
        .map(|s| s.base_issue)
        .collect();


    let all_issues = pathway_with_invalid_references_issue.into_iter().chain(validate_by_station(gtfs, custom_rules)).collect();
    all_issues


}

pub fn validate_by_station(gtfs: &gtfs_structures::Gtfs,custom_rules: &CustomRules) -> Vec<Issue> {
    // Pre-compute shared read-only data structures ONCE
    let stations_and_their_children = get_stations_and_their_children(gtfs);
    let connected_stations = get_unique_stations_connected_by_pathways(gtfs);
    let platforms_with_boarding_areas = get_platforms_with_boarding_areas(gtfs);
    let all_stops = &gtfs.stops;

    // Run all 7 validators in parallel using rayon::scope
    // We use a Mutex-protected HashMap to collect results from parallel tasks,
    // since rayon::scope spawns return () not JoinHandle.
    use std::sync::Mutex;
    let all_issues_by_station = Mutex::new(HashMap::<String, Vec<PathwayIssue>>::new());
    rayon::scope(|s| {
        s.spawn(|_| {
            let result = get_dangling_stop_validation_issues_by_station(&connected_stations, &stations_and_their_children);
            if !result.is_empty() {
                let mut map = all_issues_by_station.lock().unwrap();
                for (station_id, issues) in result {
                    map.entry(station_id).or_default().extend(issues);
                }
            }
        });
        s.spawn(|_| {
            let result = get_transitive_level_inconsistencies_by_station(&connected_stations, &stations_and_their_children,custom_rules.max_allowed_ramp_slope.unwrap_or(0.06), custom_rules.require_walkway_slope);
            if !result.is_empty() {
                let mut map = all_issues_by_station.lock().unwrap();
                for (station_id, issues) in result {
                    map.entry(station_id).or_default().extend(issues);
                }
            }
        });
        s.spawn(|_| {
            let result = get_locked_platform_validation_issues_by_station(&connected_stations, &stations_and_their_children, custom_rules.max_allowed_ramp_slope.unwrap_or(0.06), custom_rules.require_walkway_slope);
            if !result.is_empty() {
                let mut map = all_issues_by_station.lock().unwrap();
                for (station_id, issues) in result {
                    map.entry(station_id).or_default().extend(issues);
                }
            }
        });
        s.spawn(|_| {
            let result = get_falsely_labelled_wheelchair_accessible_issues_by_station(&connected_stations, &stations_and_their_children, custom_rules.max_allowed_ramp_slope.unwrap_or(0.06), custom_rules.require_walkway_slope);
            if !result.is_empty() {
                let mut map = all_issues_by_station.lock().unwrap();
                for (station_id, issues) in result {
                    map.entry(station_id).or_default().extend(issues);
                }
            }
        });
        s.spawn(|_| {
            let result = get_pathways_inside_station_with_foreign_ancestor_issues_by_station(&connected_stations, all_stops);
            if !result.is_empty() {
                let mut map = all_issues_by_station.lock().unwrap();
                for (station_id, issues) in result {
                    map.entry(station_id).or_default().extend(issues);
                }
            }
        });
        s.spawn(|_| {
            let result = get_pathways_with_incompatible_level_issues_by_stattion(&connected_stations, all_stops);
            if !result.is_empty() {
                let mut map = all_issues_by_station.lock().unwrap();
                for (station_id, issues) in result {
                    map.entry(station_id).or_default().extend(issues);
                }
            }
        });
        s.spawn(|_| {
            let result = get_pathways_span_exceeding_threshold_issues_by_station(&connected_stations, all_stops, custom_rules);
            if !result.is_empty() {
                let mut map = all_issues_by_station.lock().unwrap();
                for (station_id, issues) in result {
                    map.entry(station_id).or_default().extend(issues);
                }
            }
        });
        s.spawn(|_| {
            let result = get_pathway_touching_platforms_with_boarding_areas_issues_by_station(&connected_stations, &platforms_with_boarding_areas);
            if !result.is_empty() {
                let mut map = all_issues_by_station.lock().unwrap();
                for (station_id, issues) in result {
                    map.entry(station_id).or_default().extend(issues);
                }
            }
        });
    });
    let all_issues_by_station = all_issues_by_station.into_inner().unwrap();


    // Build StationWithPathwayIssue for each station that has issues
    let mut stations_and_problems: Vec<StationWithPathwayIssue> = Vec::new();
    for (station_id, issues) in all_issues_by_station {
        let coordinates = gtfs.stops.get(&station_id)
            .and_then(|s| match (s.longitude, s.latitude) {
                (Some(lon), Some(lat)) => Some(Coord { x: lon, y: lat }),
                _ => None,
            });
        let children = stations_and_their_children.get(&station_id).cloned().unwrap_or_default();
        let edges: Vec<Arc<Pathway>> = children.iter()
            .flat_map(|c| c.pathways.iter().map(|p| Arc::new(p.clone())))
            .collect();

        let mut station_with_issues = StationWithPathwayIssue::new(station_id, coordinates, children, edges);
        for issue in issues {
            // For PathwayIncompatibleAncestor issues, collect foreign stops
            // (stops that don't belong to this station) so they can be drawn on the map.
            if issue.base_issue.issue_type == IssueType::PathwayIncompatibleAncestor {
                for stop_id in &issue.involved_stop_ids {
                    // Find the stop's ancestor; if it's different from this station, it's foreign
                    if let Some(stop) = gtfs.stops.get(stop_id) {
                        let ancestor = get_oldest_ancestor(stop, &gtfs.stops);
                        if ancestor.as_ref().map(|a| &a.id) != Some(&station_with_issues.station_id) {
                            station_with_issues.add_foreign_stop(stop.clone());
                        }
                    }
                }
            }
            station_with_issues.add_issue(issue);
        }
        stations_and_problems.push(station_with_issues);
    }


    if let Ok(json) = serde_json::to_string_pretty(&stations_and_problems) {
        let _ = std::fs::write("stations_and_issues.json", &json);
    }

    stations_and_problems
        .into_iter()
        .flat_map(|s| s.pathway_issues.into_iter().map(|pi| pi.base_issue))
        .collect()
}


fn validate_ancestor_of_pathways(gtfs: &gtfs_structures::Gtfs) -> Vec<PathwayIssue> {
    gtfs.stops
        .values()
        .collect::<Vec<_>>()          // collect into Vec for par_iter
        .par_iter()                    // parallel iterator over &Arc<Stop>
        .flat_map_iter(|stop_arc| stop_arc.as_ref().pathways.iter())
        .filter(|pathway| !pathway_connecting_stops_with_same_ancestor(pathway, &gtfs.stops))
        .map(make_no_common_ancestor_for_pathway_issue)
        .collect()
}

fn get_pathways_with_incompatible_level_issues_by_stattion(connected_stations: &HashMap<String, Vec<Arc<Stop>>>, all_stops: &HashMap<String, Arc<Stop>>) -> HashMap<String, Vec<PathwayIssue>> {
    connected_stations
        .par_iter()
        .filter_map(|(station_id, children_inside_connected_station)| {
            let pathways_with_incompatible_ancestor: Vec<PathwayIssue> = children_inside_connected_station
                .iter()
                .flat_map(|stop_arc| stop_arc.as_ref().pathways.iter())
                .filter(|pathway| !pathway_has_compatible_levels(pathway, all_stops))
                .map( make_pathway_doesnt_have_compatible_level_issue)
                .collect();
            if pathways_with_incompatible_ancestor.is_empty() {
                None
            } else {
                Some((station_id.clone(), pathways_with_incompatible_ancestor))
            }
        })
        .collect()
}
fn get_pathways_inside_station_with_foreign_ancestor_issues_by_station(connected_stations: &HashMap<String, Vec<Arc<Stop>>>, all_stops: &HashMap<String, Arc<Stop>>) -> HashMap<String, Vec<PathwayIssue>> {
    connected_stations
        .par_iter()
        .filter_map(|(station_id, children_inside_connected_station)| {
            let pathways_with_incompatible_ancestor: Vec<PathwayIssue> = children_inside_connected_station
                .iter()
                .flat_map(|stop_arc| stop_arc.as_ref().pathways.iter())
                .filter(|pathway| !pathway_connecting_stops_with_same_ancestor(pathway, all_stops))
                .map( make_no_common_ancestor_for_pathway_issue)
                .collect();
            if pathways_with_incompatible_ancestor.is_empty() {
                None
            } else {
                Some((station_id.clone(), pathways_with_incompatible_ancestor))
            }
        })
        .collect()
}

fn get_pathway_touching_platforms_with_boarding_areas_issues_by_station(connected_stations: &HashMap<String, Vec<Arc<Stop>>>, platforms_with_boarding_areas: &Vec<String>) -> HashMap<String, Vec<PathwayIssue>> {
    connected_stations
        .par_iter()
        .filter_map(|(station_id, children_inside_connected_station)| {
            let pathways_touching_boarding_areas: Vec<PathwayIssue> = children_inside_connected_station
                .iter()
                .flat_map(|stop_arc| stop_arc.as_ref().pathways.iter())
                .filter_map(|pathway| {
                    pathway_connected_to_platform_with_boarding_areas(
                        pathway,
                        platforms_with_boarding_areas,
                    )
                        .map(|reason| (pathway, reason))
                })
                .map(|(pathway, reason)| {
                    make_pathway_to_platform_with_boarding_areas_issue(pathway, reason)
                })
                .collect();
            if pathways_touching_boarding_areas.is_empty() {
                None
            } else {
                Some((station_id.clone(), pathways_touching_boarding_areas))
            }
        })
        .collect()
}

fn get_pathways_span_exceeding_threshold_issues_by_station(connected_stations: &HashMap<String, Vec<Arc<Stop>>>, all_stops: &HashMap<String, Arc<Stop>>, custom_rules: &CustomRules) -> HashMap<String, Vec<PathwayIssue>> {
    let Some(threshold) = custom_rules.max_distance_spanned_by_pathway else {
        return HashMap::new();
    };

    connected_stations
        .par_iter()
        .filter_map(|(station_id, children_inside_connected_station)| {
            let pathways_with_incompatible_ancestor: Vec<PathwayIssue> = children_inside_connected_station
                .iter()
                .flat_map(|stop_arc| stop_arc.as_ref().pathways.iter())
                .filter(|pathway| pathway_spanning_distance_above_threshold(pathway, all_stops, threshold))
                .map( |pathway| make_pathways_connecting_stops_too_far_issue(pathway, &threshold))
                .collect();
            if pathways_with_incompatible_ancestor.is_empty() {
                None
            } else {
                Some((station_id.clone(), pathways_with_incompatible_ancestor))
            }
        })
        .collect()
}

fn get_dangling_stop_validation_issues_by_station(connected_stations: &HashMap<String, Vec<Arc<Stop>>>, stations_and_their_children: &HashMap<String, Vec<Arc<Stop>>>)->HashMap<String,Vec<PathwayIssue>>{

    let dangling_stops = get_all_stops_in_connected_station_not_connected_by_pathways(connected_stations, stations_and_their_children);
    if dangling_stops.is_empty() {
        return HashMap::new()
    }

    dangling_stops
        .into_par_iter()
        .flat_map(|(parent_stop, dangling_children)| {
            dangling_children
                .into_iter()
                .map(|dangling_child| {
                    let new_dangling_issue = make_dangling_stop_issue(parent_stop.clone(), dangling_child.id.clone());
                    (parent_stop.clone(), new_dangling_issue)
                })
                .collect::<Vec<_>>()
        })
        .fold(
            HashMap::new,
            |mut map: HashMap<String, Vec<PathwayIssue>>, (parent_stop, issue)| {
                map.entry(parent_stop).or_default().push(issue);
                map
            },
        )
        .reduce_with(|mut a, mut b| {
            for (k, mut v) in b {
                a.entry(k).or_default().append(&mut v);
            }
            a
        })
        .unwrap_or_default()

}


fn get_transitive_level_inconsistencies_by_station(connected_stations: &HashMap<String, Vec<Arc<Stop>>>, stations_and_their_children: &HashMap<String, Vec<Arc<Stop>>>, max_allowed_ramp_slope: f64, require_walkway_slope: bool) -> HashMap<String,Vec<PathwayIssue>> {

    connected_stations
        .par_iter()
        .filter_map(|(station_id, _connected_children)| {
            let children = stations_and_their_children.get(station_id)?;

            let mut graph_of_station = PathwayDirectedGraph::new(max_allowed_ramp_slope, require_walkway_slope);
            for child in children {
                graph_of_station.add_stop(&child);
            }

            let mut issues_in_station: Vec<PathwayIssue> = Vec::new();

            for child in children {
                // A = child.id  (the source stop)
                // B = a stop reachable from A only via elevator/stairs (no direct walkway from A)
                // C = a stop reachable from B via a direct walkway
                // Inconsistency: C is also reachable from A via a direct walkway,
                // because elevator/stairs should imply a level change that walkways shouldn't bypass.

                let Some(elev_or_stairs_from_a) = graph_of_station.nodes_reachable_by_elevator_or_stairs.get(&child.id) else {
                    continue;
                };
                let walkway_from_a = graph_of_station.nodes_reachable_by_walkway.get(&child.id);

                for b in elev_or_stairs_from_a {
                    // B must NOT be reachable from A via a direct walkway
                    if walkway_from_a.is_some_and(|w| w.contains(b)) {
                        continue;
                    }

                    let Some(walkway_from_b) = graph_of_station.nodes_reachable_by_walkway.get(b) else { continue };

                    for c in walkway_from_b {
                        // C must be reachable from A via a direct walkway (the inconsistency)
                        if walkway_from_a.is_some_and(|w| w.contains(c)) {
                            let message:String = format!("stops {},{},{} form an intransitive triad", child.id, b, c);

                            let transitive_inconsistency_issue = make_transitive_level_inconsistency_issue(station_id.clone(), message,[child.id.clone(), b.clone(), c.clone()].map(String::from).to_vec());
                            issues_in_station.push(transitive_inconsistency_issue);
                        }
                    }
                }


            }

            // Deduplicate: keep only one issue per unique set of involved stops.
            // The algorithm may detect the same triad from different starting points,
            // producing rotated versions like (A,B,C), (B,C,A), (C,A,B).
            // We normalize by sorting the involved stop IDs and keeping the first occurrence.
            let mut seen_triads: HashSet<Vec<String>> = HashSet::new();
            let unique_issues: Vec<PathwayIssue> = issues_in_station
                .into_iter()
                .filter(|issue| {
                    let mut canonical = issue.involved_stop_ids.clone();
                    canonical.sort();
                    seen_triads.insert(canonical)
                })
                .collect();
            if unique_issues.is_empty() { None } else { Some((station_id.clone(), unique_issues)) }
        })
        .collect()
}


fn get_locked_platform_validation_issues_by_station(connected_stations: &HashMap<String, Vec<Arc<Stop>>>, stations_and_their_children: &HashMap<String, Vec<Arc<Stop>>>, max_allowed_ramp_slope: f64, require_walkway_slope: bool)->HashMap<String,Vec<PathwayIssue>>{
    let locked = get_all_island_stops_in_connected_station(connected_stations, stations_and_their_children, max_allowed_ramp_slope, require_walkway_slope);
    if locked.is_empty() {
        return HashMap::new()
    }

    locked
        .into_par_iter()
        .flat_map(|(parent_stop, locked_children)| {
            locked_children
                .into_iter()
                .map(|child| {
                    let new_locked_platform_issue = make_locked_platform_issue(parent_stop.clone(), child.id.clone());
                    (parent_stop.clone(), new_locked_platform_issue)
                })
                .collect::<Vec<_>>()
        })
        .fold(
            HashMap::new,
            |mut map: HashMap<String, Vec<PathwayIssue>>, (parent_stop, issue)| {
                map.entry(parent_stop).or_default().push(issue);
                map
            },
        )
        .reduce_with(|mut a, mut b| {
            for (k, mut v) in b {
                a.entry(k).or_default().append(&mut v);
            }
            a
        })
        .unwrap_or_default()
}

fn get_pathways_with_invalid_stop_references(gtfs:&Gtfs)->Vec<PathwayIssue>{

    let pathways: Vec<&Pathway> = gtfs.pathways.values().collect();
    pathways
        .par_iter()
        .filter(|p| !pathway_has_valid_stop_references(p, &gtfs.stops))
        .map(|p| make_pathways_referencing_no_existent_stops(p))
        .collect()

}

fn get_falsely_labelled_wheelchair_accessible_issues_by_station(connected_stations: &HashMap<String, Vec<Arc<Stop>>>, stations_and_their_children: &HashMap<String, Vec<Arc<Stop>>>, max_allowed_ramp_slope: f64, require_walkway_slope: bool)->HashMap<String,Vec<PathwayIssue>>{
    let mislabeled_stops = get_all_falsely_accessible_labelled__stops_in_connected_station(connected_stations, stations_and_their_children, max_allowed_ramp_slope, require_walkway_slope);
    if mislabeled_stops.is_empty() {
        return HashMap::new()
    }

    mislabeled_stops
        .into_par_iter()
        .flat_map(|(parent_stop, mislabeled_children)| {
            mislabeled_children
                .into_iter()
                .map(|mislabeled_stop| {
                    let mislabeled_stop_issue = make_falsely_labelled_wheelchair_accessible_issue(parent_stop.clone(), mislabeled_stop.id.clone());
                    (parent_stop.clone(), mislabeled_stop_issue)
                })
                .collect::<Vec<_>>()
        })
        .fold(
            HashMap::new,
            |mut map: HashMap<String, Vec<PathwayIssue>>, (parent_stop, issue)| {
                map.entry(parent_stop).or_default().push(issue);
                map
            },
        )
        .reduce_with(|mut a, mut b| {
            for (k, mut v) in b {
                a.entry(k).or_default().append(&mut v);
            }
            a
        })
        .unwrap_or_default()
}




fn pathway_has_valid_stop_references(pathway: &Pathway,all_stops: &HashMap<String, Arc<Stop>>)->bool{
    let (Some(from_stop), Some(to_stop)) = (
        all_stops.get(&pathway.from_stop_id).map(Arc::as_ref),
        all_stops.get(&pathway.to_stop_id).map(Arc::as_ref),
    ) else {
        return false;
    };

     all_stops.contains_key(from_stop.id()) && all_stops.contains_key(to_stop.id())
}

fn pathway_connecting_stops_with_same_ancestor(
    pathway: &Pathway,
    all_stops: &HashMap<String, Arc<Stop>>,
) -> bool {
    let (Some(from_stop), Some(to_stop)) = (
        all_stops.get(&pathway.from_stop_id).map(Arc::as_ref),
        all_stops.get(&pathway.to_stop_id).map(Arc::as_ref),
    ) else {
        return false;
    };

    let ancestor_of_origin = get_oldest_ancestor(from_stop, all_stops);
    let ancestor_of_source = get_oldest_ancestor(to_stop, all_stops);

    match (ancestor_of_origin.as_deref(), ancestor_of_source.as_deref()) {
        (Some(x), Some(y)) => x.id == y.id,
        _ => false,
    }
}

fn pathway_spanning_distance_above_threshold(pathway: &Pathway,all_stops: &HashMap<String, Arc<Stop>>, threshold: f64) -> bool {
    let from_stop= all_stops.get(&pathway.from_stop_id).map(Arc::as_ref);
    let to_stop= all_stops.get(&pathway.to_stop_id).map(Arc::as_ref);

    match (from_stop,to_stop) {
        (Some(from_stop), Some(to_stop)) => {
            stops_too_far(from_stop, to_stop, threshold)
        },
        _ => false,
    }
}

pub fn pathway_has_compatible_levels(pathway:& Pathway,all_stops: &HashMap<String, Arc<Stop>>)->bool{

    let from = all_stops.get(&pathway.from_stop_id).unwrap().as_ref();
    let to = all_stops.get(&pathway.to_stop_id).unwrap().as_ref();

    match (  &from.level_id, &to.level_id) {
        (Some(_), Some(_)) => {
            match pathway.mode {
                PathwayMode::Elevator=> ! stops_at_same_level(from, to),
                PathwayMode::Escalator=> ! stops_at_same_level(from, to),
                PathwayMode::Stairs=> ! stops_at_same_level(from, to),
                PathwayMode::MovingSidewalk =>  stops_at_same_level(from, to),
                _ => true
            }

    }

        _ => true
    }
}

fn get_oldest_ancestor(
    stop: &Stop,
    all_stops: &HashMap<String, Arc<Stop>>,
) -> Option<Arc<Stop>> {
    match stop.location_type {
        // Your map already holds an Arc for every stop, so look ourselves up.
        LocationType::StopArea => all_stops.get(&stop.id).cloned(),

        LocationType::StationEntrance
        | LocationType::StopPoint
        | LocationType::GenericNode => {
            let mut current = stop
                .parent_station
                .as_deref()
                .and_then(|p| all_stops.get(p));
            while let Some(node) = current {
                if node.location_type == LocationType::StopArea {
                    return Some(node.clone());
                }
                current = node
                    .parent_station
                    .as_deref()
                    .and_then(|p| all_stops.get(p));
            }
            // Fall back to immediate parent if no StopArea found
            stop.parent_station
                .as_deref()
                .and_then(|p| all_stops.get(p))
                .cloned()
        }

        LocationType::BoardingArea => {
            let immediate_parent = stop
                .parent_station
                .as_deref()
                .and_then(|p| all_stops.get(p))?;


            immediate_parent
                .parent_station
                .as_deref()
                .and_then(|p| all_stops.get(p))
                .cloned()
        }

        LocationType::Unknown(_) => None,
    }
}


fn get_all_stops_in_connected_station_not_connected_by_pathways(
    connected_stations: &HashMap<String, Vec<Arc<Stop>>>,
    stations_and_their_children: &HashMap<String, Vec<Arc<Stop>>>,
) -> HashMap<String, Vec<Arc<Stop>>> {

    connected_stations
        .par_iter()
        .filter_map(|(station_id, connected_children)| {
            let connected_ids: HashSet<&str> = connected_children
                .iter()
                .map(|s| s.id.as_str())
                .collect();

            let children = stations_and_their_children.get(station_id)?;

            let missing: Vec<Arc<Stop>> = children
                .iter()
                .filter(|s| s.location_type != LocationType::BoardingArea && !(s.location_type==LocationType::StopPoint && s.stop_access==StopAccess::DirectlyAccessible) && !connected_ids.contains(s.id.as_str()))
                .cloned()
                .collect();

            if missing.is_empty() {
                None
            } else {
                Some((station_id.clone(), missing))
            }
        })
        .collect()
}

fn get_all_island_stops_in_connected_station(
    connected_stations: &HashMap<String, Vec<Arc<Stop>>>,
    stations_and_their_children: &HashMap<String, Vec<Arc<Stop>>>,
    max_allowed_ramp_slope: f64,
    require_walkway_slope: bool,
) -> HashMap<String, Vec<Arc<Stop>>> {

    connected_stations
        .par_iter()
        .filter_map(|(station_id, _connected_children)| {
            let children = stations_and_their_children.get(station_id)?;

            let platforms_entrances_of_station = get_platforms_and_boarding_areas_and_entrances_for_station(children);
            let mut graph_of_station = PathwayDirectedGraph::new(max_allowed_ramp_slope, require_walkway_slope);
            for child in children {
                graph_of_station.add_stop(&child);
            }

            let entries = platforms_entrances_of_station.1;

            let islands: Vec<Arc<Stop>> = children
                .iter()
                .filter(|s| is_a_platform(s))
                .filter(|s| !(s.stop_access == StopAccess::DirectlyAccessible) &&!stop_reachable_from_at_least_one_entrance(s,&entries,&graph_of_station))
                .cloned()
                .collect();

            if islands.is_empty() {
                None
            } else {
                Some((station_id.clone(), islands))
            }
        })
        .collect()
}

fn get_all_falsely_accessible_labelled__stops_in_connected_station(
    connected_stations: &HashMap<String, Vec<Arc<Stop>>>,
    stations_and_their_children: &HashMap<String, Vec<Arc<Stop>>>,
    max_allowed_ramp_slope: f64,
    require_walkway_slope: bool,
) -> HashMap<String, Vec<Arc<Stop>>> {

    connected_stations
        .par_iter()
        .filter_map(|(station_id, _connected_children)| {
            let children = stations_and_their_children.get(station_id)?;

            let platforms_entrances_of_station = get_platforms_and_boarding_areas_and_entrances_for_station(children);
            let mut graph_of_station = PathwayDirectedGraph::new(max_allowed_ramp_slope, require_walkway_slope);
            for child in children {
                graph_of_station.add_stop(&child);
            }

            let entries = platforms_entrances_of_station.1;

            let false_labelled: Vec<Arc<Stop>> = children
                .iter()
                .filter(|s| is_a_platform(s))
                .filter(|s| s.wheelchair_boarding == Availability::Available)
                .filter(|s| !stop_reachable_from_al_least_one_entrance_for_wheelchair_users(s, &entries, &graph_of_station))
                .cloned()
                .collect();

            if false_labelled.is_empty() {
                None
            } else {
                Some((station_id.clone(), false_labelled))
            }
        })
        .collect()
}

fn is_a_platform(stop:&Stop) -> bool {
    stop.location_type == LocationType::StopPoint || stop.location_type == LocationType::BoardingArea
}
fn stop_reachable_from_at_least_one_entrance(stop:&Stop,entrances: &Vec<Arc<Stop>>,graph:&PathwayDirectedGraph)-> bool {
    let mut is_reachable= false;
    for entrance in entrances {
        let is_reachable_from_entrance = graph.is_reachable(&*entrance.id, &*stop.id);
        if is_reachable_from_entrance {
            is_reachable = true;
            break
        }
    }
    is_reachable
}

fn stop_reachable_from_al_least_one_entrance_for_wheelchair_users(stop:&Stop,entrances: &Vec<Arc<Stop>>,graph:&PathwayDirectedGraph)->bool{
    let mut is_reachable= false;
    for entrance in entrances {
        let is_reachable_from_entrance = graph.is_reachable_by_wheelchair(&*entrance.id, &*stop.id);
        if is_reachable_from_entrance {
            is_reachable = true;
            break
        }
    }
    is_reachable
}

fn get_platforms_and_boarding_areas_and_entrances_for_station(children:&Vec<Arc<Stop>>) -> (Vec<Arc<Stop>>,Vec<Arc<Stop>>) {
 let platforms_and_boarding_areas= children.iter().filter(|s| s.location_type == LocationType::BoardingArea || s.location_type==LocationType::StopPoint).cloned().collect();
 let entrances = children.iter().filter(|s| s.location_type == LocationType::StationEntrance).cloned().collect();
 (platforms_and_boarding_areas, entrances)
}

fn get_stations_and_their_children(gtfs: &Gtfs) -> HashMap<String, Vec<Arc<Stop>>> {
    let mut by_station: HashMap<String, Vec<Arc<Stop>>> = HashMap::new();
    for stop in gtfs.stops.values() {
        if stop.location_type == LocationType::StopArea {
            continue;
        }
        let Some(station) = get_oldest_ancestor(stop, &gtfs.stops) else {
            continue;
        };
        by_station
            .entry(station.id.clone())
            .or_default()
            .push(stop.clone());
    }
    by_station
}

fn get_unique_stations_connected_by_pathways(
    gtfs: &Gtfs,
) -> HashMap<String, Vec<Arc<Stop>>> {
    // Every stop id that is either endpoint of any pathway.
    let connected_ids: HashSet<&str> = gtfs
        .stops
        .values()
        .flat_map(|s| s.pathways.iter())
        .flat_map(|p| [p.from_stop_id.as_str(), p.to_stop_id.as_str()])
        .collect();

    let mut by_station: HashMap<String, Vec<Arc<Stop>>> = HashMap::new();

    for stop in gtfs.stops.values() {
        if !connected_ids.contains(stop.id.as_str()) {
            continue;
        }
        let Some(station) = get_oldest_ancestor(stop, &gtfs.stops) else {
            continue;
        };
        let entry = by_station
            .entry(station.id.clone())
            .or_default();
        // A stop may appear as an endpoint of multiple pathways, so guard against duplicates.
        if !entry.iter().any(|s| s.id == stop.id) {
            entry.push(stop.clone());
        }
    }

    by_station
}

fn get_platforms_with_boarding_areas(gtfs: &Gtfs) -> Vec<String> {
    gtfs.stops
        .values()
        .collect::<Vec<_>>()
        .par_iter()
        .filter(|s| s.location_type == LocationType::BoardingArea)
        .filter_map(|boarding_area| {
            boarding_area.parent_station.clone()})
        .collect()
}

fn pathway_connected_to_platform_with_boarding_areas(pathway: &Pathway,platforms_with_boarding_areas :&Vec<String>)->Option<String> {
    let from_id = pathway.clone().from_stop_id;
    let to_id = pathway.clone().to_stop_id;
    let is_from_id_invalid = platforms_with_boarding_areas.contains(&from_id);
    let is_to_id_invalid = platforms_with_boarding_areas.contains(&to_id);
    if is_from_id_invalid {
         return Some(from_id.clone());
    };
    if is_to_id_invalid {
        return Some(to_id.clone());
    };
    None
}
fn stops_too_far(stop_a: &gtfs_structures::Stop, stop_b: &gtfs_structures::Stop,threshold:f64) -> bool {

    match (
        stop_a.longitude,
        stop_a.latitude,
        stop_b.longitude,
        stop_b.latitude,
    ) {
        (Some(lon_a), Some(lat_a), Some(lon_b), Some(lat_b)) => {
            let a = Point::new(lon_a, lat_a);
            let b = Point::new(lon_b, lat_b);

            if(Haversine.distance(a,b)>threshold){
                true
            }else { false }
        }
        _ => false,
    }
}

fn make_no_common_ancestor_for_pathway_issue(pathway: &Pathway) -> PathwayIssue {
    let message = format!("the pathway with id {}   connects stops {} to stop {} having different ancestor ", pathway.id, pathway.from_stop_id, pathway.to_stop_id);

    let base_issue = Issue::new(Severity::Error, IssueType::PathwayIncompatibleAncestor, &pathway.id).details(message.as_str());

    let mut involved_stops:Vec<String> = Vec::new();
    involved_stops.push(pathway.from_stop_id.clone());
    involved_stops.push(pathway.to_stop_id.clone());
    PathwayIssue::new(base_issue, Some(pathway.id.clone()), involved_stops)}

fn make_pathways_connecting_stops_too_far_issue(pathway: &gtfs_structures::Pathway,theshold:&f64) -> PathwayIssue {
    let message = format!("the pathway with id {}   connects stops {} to stop {} spans a distance more than the user provided threshold {} meters", pathway.id, pathway.from_stop_id, pathway.to_stop_id,theshold);

    let base_issue = Issue::new(Severity::Error,IssueType::PathwayConnectingStopsTooFar,&pathway.id).details(message.as_str());

    let mut involved_stops:Vec<String> = Vec::new();
    involved_stops.push(pathway.from_stop_id.clone());
    involved_stops.push(pathway.to_stop_id.clone());
    PathwayIssue::new(base_issue, Some(pathway.id.clone()), involved_stops)

}

fn make_pathways_referencing_no_existent_stops(pathway: &gtfs_structures::Pathway) -> PathwayIssue {
    let message = format!("Pathway with id {} references no existent stops",pathway.id.clone());
    let base_issue = Issue::new(Severity::Error,IssueType::PathwayHasInvalidStopId,&pathway.id).details(message.as_str());

    let mut involved_stops:Vec<String> = Vec::new();
    involved_stops.push(pathway.from_stop_id.clone());
    involved_stops.push(pathway.to_stop_id.clone());
    PathwayIssue::new(base_issue, Some(pathway.id.clone()), involved_stops)

}


fn make_pathway_doesnt_have_compatible_level_issue(pathway: &Pathway)->PathwayIssue {
    let mode = format!("Pathway mode is {:?}", pathway.mode);
    let message = format!("the pathway with id {}   connects stops {} to stop {} is incompatible with mode {} ", pathway.id, pathway.from_stop_id, pathway.to_stop_id,mode);
    let base_issue = Issue::new(Severity::Error, IssueType::PathwayModeNotCompatibleWithLevels, &pathway.id).details(message.as_str());

    let mut involved_stops:Vec<String> = Vec::new();
    involved_stops.push(pathway.from_stop_id.clone());
    involved_stops.push(pathway.to_stop_id.clone());
    PathwayIssue::new(base_issue,Some(pathway.id.clone()),involved_stops)
}

fn make_dangling_stop_issue(parent_stop_id:String,dangling_stop_id:String)->PathwayIssue {

    let message = format!("Station with id {} has a dangling stop with id {}", parent_stop_id, dangling_stop_id);
    let base_issue = Issue::new(Severity::Error,IssueType::DanglingStop,&dangling_stop_id).details(message.as_str());
    let mut involved_stops = Vec::new();
    involved_stops.push(dangling_stop_id);

    PathwayIssue::new_stop_issue(base_issue,involved_stops)
}

fn make_locked_platform_issue(parent_stop_id:String,locked_stop_id:String)->PathwayIssue {
    let message = format!("Station with id {} has a dangling stop with id {}", parent_stop_id, locked_stop_id);
    let base_issue = Issue::new(Severity::Error,IssueType::LockedPlatform,&locked_stop_id).details(message.as_str());

    let mut involved_stops = Vec::new();
    involved_stops.push(locked_stop_id);
    PathwayIssue::new_stop_issue(base_issue,involved_stops)
}

fn make_transitive_level_inconsistency_issue(station_id:String, message:String,involved_stops:Vec<String>) ->PathwayIssue {
    let base_issue = Issue::new(Severity::Warning,PathwaysImpliesTransitiveInconsistency, &station_id).details(message.as_str()).details(message.as_str());
    let involved_stops = involved_stops;
    PathwayIssue::new_stop_issue(base_issue,involved_stops)
}

fn make_falsely_labelled_wheelchair_accessible_issue(parent_stop_id:String, mislabelled_stop_id:String) ->PathwayIssue {
    let message = format!("Station with id {} has inconsistent wheelchair availability labelling for  stop with id {}", parent_stop_id, mislabelled_stop_id);
    let base_issue = Issue::new(Severity::Warning,IssueType::WheelchairBoardingLabelInconsistentWithPathways,&mislabelled_stop_id).details(message.as_str());

    let mut involved_stops = Vec::new();
    involved_stops.push(mislabelled_stop_id);
    PathwayIssue::new_stop_issue(base_issue,involved_stops)
}

fn make_pathway_to_platform_with_boarding_areas_issue(pathway: &gtfs_structures::Pathway,problem_station_id:String)->PathwayIssue {
    let message = format!("pathway with id {} is connected to location with id {} that contains boarding areas",pathway.id, problem_station_id);
    let base_issue= Issue::new(Severity::Error,IssueType::PathwayToPlatformWithBoardingAreas,&problem_station_id).details(message.as_str());

    let mut involved_stops : Vec<String> = Vec::new();
    involved_stops.push(problem_station_id.clone());
    PathwayIssue::new(base_issue, Some(pathway.id.clone()), involved_stops)
}

fn stops_at_same_level(start: & Stop,end: & Stop )->bool{
    start.level_id == end.level_id
}



#[test]
fn test_ancestor_detection() {
    let main_station = gtfs_structures::Stop {
        id: String::from("main_station00"),
        parent_station: None,
        location_type: LocationType::StopArea,
        code: None,
        latitude: Some(50.22),
        longitude: Some(6.022),
        description: None,
        zone_id: None,
        url: None,
        level_id: Some(String::from("1")),
        platform_code: None,
        pathways: Vec::new(),
        transfers: Vec::new(),
        name: None,
        timezone: None,
        tts_name: None,
        wheelchair_boarding: Availability::InformationNotAvailable,
        stop_access: Default::default(),
    };


    let platform_one = gtfs_structures::Stop {
        id: String::from("platform_one00"),
        parent_station: Some(main_station.id.clone()),
        location_type: LocationType::StopPoint,
        code: None,
        latitude: Some(50.22),
        longitude: Some(6.022),
        description: None,
        zone_id: None,
        url: None,
        level_id: Some(String::from("1")),
        platform_code: None,
        pathways: Vec::new(),
        transfers: Vec::new(),
        name: None,
        timezone: None,
        tts_name: None,
        wheelchair_boarding: Availability::InformationNotAvailable,
        stop_access: Default::default(),
    };

    let platform_two = gtfs_structures::Stop {
        id: String::from("platform_two00"),
        parent_station: Some(main_station.id.clone()),
        location_type: LocationType::StopPoint,
        code: None,
        latitude: Some(50.22),
        longitude: Some(6.022),
        description: None,
        zone_id: None,
        url: None,
        level_id: Some(String::from("1")),
        platform_code: None,
        pathways: Vec::new(),
        transfers: Vec::new(),
        name: None,
        timezone: None,
        tts_name: None,
        wheelchair_boarding: Availability::InformationNotAvailable,
        stop_access: Default::default(),
    };

    let boarding_area_platform_two = gtfs_structures::Stop {
        id: String::from("platform_two00_boarding_area"),
        parent_station: Some(platform_two.id.clone()),
        location_type: LocationType::BoardingArea,
        code: None,
        latitude: Some(50.22),
        longitude: Some(6.022),
        description: None,
        zone_id: None,
        url: None,
        level_id: Some(String::from("1")),
        platform_code: None,
        pathways: Vec::new(),
        transfers: Vec::new(),
        name: None,
        timezone: None,
        tts_name: None,
        wheelchair_boarding: Availability::InformationNotAvailable,
        stop_access: Default::default(),
    };

    let mut all_stops: HashMap<String, Arc<Stop>> = HashMap::new();
    all_stops.insert(String::from("main_station00"), Arc::new(main_station.clone()));
    all_stops.insert(String::from("platform_one00"), Arc::new(platform_one.clone()));
    all_stops.insert(String::from("platform_two00"), Arc::new(platform_two.clone()));

    let ancestor_of_main_station = get_oldest_ancestor(&main_station, &all_stops);
    assert!(ancestor_of_main_station.is_some());

    let ancestor_of_platform_one = get_oldest_ancestor(&platform_one, &all_stops);
    assert!(ancestor_of_platform_one.is_some());
    assert_eq!(ancestor_of_platform_one.unwrap().id, "main_station00");

    let ancestor_of_platform_two = get_oldest_ancestor(&platform_two, &all_stops);
    assert!(ancestor_of_platform_two.is_some());
    assert_eq!(ancestor_of_platform_two.unwrap().id, "main_station00");


    let ancestor_of_boarding_area = get_oldest_ancestor(&boarding_area_platform_two, &all_stops);

    assert!(ancestor_of_boarding_area.is_some());
    assert_eq!(ancestor_of_boarding_area.unwrap().id, "main_station00");
}


#[test]
fn pathway_ancestor_validation_fails_when_stops_have_different_ancestors() {
    let main_station = gtfs_structures::Stop {
        id: String::from("main_station00"),
        parent_station: None,
        location_type: LocationType::StopArea,
        code: None,
        latitude: Some(50.22),
        longitude: Some(6.022),
        description: None,
        zone_id: None,
        url: None,
        level_id: Some(String::from("1")),
        platform_code: None,
        pathways: Vec::new(),
        transfers: Vec::new(),
        name: None,
        timezone: None,
        tts_name: None,
        wheelchair_boarding: Availability::InformationNotAvailable,
        stop_access: Default::default(),
    };


    let platform_one_main_station = gtfs_structures::Stop {
        id: String::from("platform_one00_main_station"),
        parent_station: Some(main_station.id.clone()),
        location_type: LocationType::StopPoint,
        code: None,
        latitude: Some(50.22),
        longitude: Some(6.022),
        description: None,
        zone_id: None,
        url: None,
        level_id: Some(String::from("1")),
        platform_code: None,
        pathways: Vec::new(),
        transfers: Vec::new(),
        name: None,
        timezone: None,
        tts_name: None,
        wheelchair_boarding: Availability::InformationNotAvailable,
        stop_access: Default::default(),
    };

    let west_bahnhof = gtfs_structures::Stop {
        id: String::from("west_bahnhof00"),
        parent_station: None,
        location_type: LocationType::StopArea,
        code: None,
        latitude: Some(50.22),
        longitude: Some(6.022),
        description: None,
        zone_id: None,
        url: None,
        level_id: Some(String::from("1")),
        platform_code: None,
        pathways: Vec::new(),
        transfers: Vec::new(),
        name: None,
        timezone: None,
        tts_name: None,
        wheelchair_boarding: Availability::InformationNotAvailable,
        stop_access: Default::default(),
    };


    let platform_one_west_bahnhof = gtfs_structures::Stop {
        id: String::from("platform_one00_west_bahnhof"),
        parent_station: Some(west_bahnhof.id.clone()),
        location_type: LocationType::StopPoint,
        code: None,
        latitude: Some(50.22),
        longitude: Some(6.022),
        description: None,
        zone_id: None,
        url: None,
        level_id: Some(String::from("1")),
        platform_code: None,
        pathways: Vec::new(),
        transfers: Vec::new(),
        name: None,
        timezone: None,
        tts_name: None,
        wheelchair_boarding: Availability::InformationNotAvailable,
        stop_access: Default::default(),
    };

    let defective_pathway = Pathway {
        id: "".to_string(),
        from_stop_id: String::from(main_station.id.clone()),

        to_stop_id: String::from(platform_one_west_bahnhof.id.clone()),
        mode: PathwayMode::Walkway,
        is_bidirectional: PathwayDirectionType::Bidirectional,
        length: None,
        traversal_time: None,
        stair_count: None,
        max_slope: None,
        min_width: None,
        signposted_as: None,
        reversed_signposted_as: None,
    };

    let mut all_stops: HashMap<String, Arc<Stop>> = HashMap::new();
    all_stops.insert(String::from("main_station00"), Arc::new(main_station.clone()));
    all_stops.insert(String::from("platform_one00"), Arc::new(platform_one_west_bahnhof.clone()));
    all_stops.insert(String::from("west_bahnhof00"), Arc::new(west_bahnhof));
    all_stops.insert(String::from(platform_one_west_bahnhof.id.clone()), Arc::new(platform_one_west_bahnhof));

    assert_eq!(pathway_connecting_stops_with_same_ancestor(&defective_pathway, &all_stops), false)
}

#[test]
fn test_validating_pathways_with_no_shared_ancestor_creates_one_issue(){
    let gtfs = gtfs_structures::Gtfs::new("test_data/pathways/pathways_ancestor_problem").unwrap();

    let issues = validate_ancestor_of_pathways(&gtfs);

    assert_eq!(issues.len(), 1);
}

#[test]
fn test_validating_consistent_pathways_creates_no_issue(){
    let gtfs = gtfs_structures::Gtfs::new("test_data/pathways/original_data").unwrap();

    let issues = validate_ancestor_of_pathways(&gtfs);

    assert_eq!(issues.len(), 0);
}


#[test]
fn stops_exceeding_threshold_marked_as_too_far(){

    let main_station = gtfs_structures::Stop {
        id: String::from("main_station00"),
        parent_station: None,
        location_type: LocationType::StopArea,
        code: None,
        latitude: Some(50.22),
        longitude: Some(6.022),
        description: None,
        zone_id: None,
        url: None,
        level_id: Some(String::from("1")),
        platform_code: None,
        pathways: Vec::new(),
        transfers: Vec::new(),
        name: None,
        timezone: None,
        tts_name: None,
        wheelchair_boarding: Availability::InformationNotAvailable,
        stop_access: Default::default(),
    };


    let platform_one_main_station = gtfs_structures::Stop {
        id: String::from("platform_one00_main_station"),
        parent_station: Some(main_station.id.clone()),
        location_type: LocationType::StopPoint,
        code: None,
        latitude: Some(40.22),
        longitude: Some(6.022),
        description: None,
        zone_id: None,
        url: None,
        level_id: Some(String::from("1")),
        platform_code: None,
        pathways: Vec::new(),
        transfers: Vec::new(),
        name: None,
        timezone: None,
        tts_name: None,
        wheelchair_boarding: Availability::InformationNotAvailable,
        stop_access: Default::default(),
    };
    let file_path = Some(String::from("test_data/custom_rules/custom_rules.yml"));
    let custom_rules = custom_rules(file_path);
    let threshold = custom_rules.max_distance_spanned_by_pathway.unwrap_or(500.0);

    assert_eq!(stops_too_far(&main_station,&platform_one_main_station,threshold), true);
}

#[test]
fn stops_below_threshold_not_marked_as_too_far(){

    let main_station = gtfs_structures::Stop {
        id: String::from("main_station00"),
        parent_station: None,
        location_type: LocationType::StopArea,
        code: None,
        latitude: Some(50.22),
        longitude: Some(6.022),
        description: None,
        zone_id: None,
        url: None,
        level_id: Some(String::from("1")),
        platform_code: None,
        pathways: Vec::new(),
        transfers: Vec::new(),
        name: None,
        timezone: None,
        tts_name: None,
        wheelchair_boarding: Availability::InformationNotAvailable,
        stop_access: Default::default(),
    };


    let platform_one_main_station = gtfs_structures::Stop {
        id: String::from("platform_one00_main_station"),
        parent_station: Some(main_station.id.clone()),
        location_type: LocationType::StopPoint,
        code: None,
        latitude: Some(50.220001),
        longitude: Some(6.022),
        description: None,
        zone_id: None,
        url: None,
        level_id: Some(String::from("1")),
        platform_code: None,
        pathways: Vec::new(),
        transfers: Vec::new(),
        name: None,
        timezone: None,
        tts_name: None,
        wheelchair_boarding: Availability::InformationNotAvailable,
        stop_access: Default::default(),
    };

    let file_path = Some(String::from("test_data/custom_rules/custom_rules.yml"));
    let custom_rules = custom_rules(file_path);
    let threshold = custom_rules.max_distance_spanned_by_pathway.unwrap_or(500.0);

    assert_eq!(stops_too_far(&main_station,&platform_one_main_station,threshold), false);
}

#[test]
fn test_validating_pathways_with_large_span_creates_issue(){
    let gtfs = gtfs_structures::Gtfs::new("test_data/pathways/pathway_span_too_large").unwrap();
    let file_path = Some(String::from("test_data/custom_rules/custom_rules.yml"));
    let custom_rules = custom_rules(file_path);
    let connected_stations = get_unique_stations_connected_by_pathways(&gtfs);
    let issues = get_pathways_span_exceeding_threshold_issues_by_station(&connected_stations, &gtfs.stops, &custom_rules);

    println!("issues: {:?}", issues);
    assert!(issues.len()>0);
}

#[test]
fn test_validating_compatible_modes_creates_issue(){
    let gtfs = gtfs_structures::Gtfs::new("test_data/pathways/pathways_multiple_issues").unwrap();
    let file_path = Some(String::from("test_data/custom_rules/custom_rules.yml"));
    let custom_rules = custom_rules(file_path);
    let connected_stations = get_unique_stations_connected_by_pathways(&gtfs);
    let issues = get_pathways_with_incompatible_level_issues_by_stattion(&connected_stations, &gtfs.stops);

    println!("issues: {:?}", issues);
    assert!(issues.len()>0);

}
#[test]
fn test_extraction_of_unique_stations_connected_by_pathways(){
    let gtfs = gtfs_structures::Gtfs::new("test_data/pathways/pathways_multiple_issues").unwrap();
    let unique_stations = get_unique_stations_connected_by_pathways(&gtfs);
    assert_eq!(unique_stations.keys().len(),2);


}

#[test]
fn test_get_stations_and_their_children(){

    let gtfs = gtfs_structures::Gtfs::new("test_data/pathways/pathways_multiple_issues").unwrap();
    let unique_stations = get_unique_stations_connected_by_pathways(&gtfs);
    let station_children_map = get_stations_and_their_children(&gtfs);
    assert_eq!(station_children_map.keys().len(),2);
    assert_eq!(station_children_map.get("station_A").unwrap().len(),11);
    assert_eq!(station_children_map.get("station_B").unwrap().len(),6);
}
#[test]
fn test_get_platforms_boarding_areas_entrances(){
    let gtfs = gtfs_structures::Gtfs::new("test_data/pathways/pathways_multiple_issues").unwrap();
    let unique_stations = get_unique_stations_connected_by_pathways(&gtfs);
    let station_children_map = get_stations_and_their_children(&gtfs);

    let children_of_station_a = station_children_map.get("station_A").unwrap();

    let platforms_entrances_of_station_a = get_platforms_and_boarding_areas_and_entrances_for_station(children_of_station_a);

    let exits_entrances_station_a = platforms_entrances_of_station_a.1;

    assert_eq!(exits_entrances_station_a.len(), 2);
}

#[test]
fn test_get_map_of_stations_with_dangling_stops(){
    let gtfs = gtfs_structures::Gtfs::new("test_data/pathways/pathways_multiple_issues").unwrap();
    let connected_stations = get_unique_stations_connected_by_pathways(&gtfs);
    let stations_and_their_children = get_stations_and_their_children(&gtfs);
    let stations_with_dangling_stops = get_all_stops_in_connected_station_not_connected_by_pathways(&connected_stations, &stations_and_their_children);
    assert_eq!(stations_with_dangling_stops.keys().len(),1);
    assert_eq!(stations_with_dangling_stops.get("station_A").unwrap().len(),1);
}

#[test]
fn test_get_map_of_stations_with_locked_platforms(){
    let gtfs = gtfs_structures::Gtfs::new("test_data/pathways/pathways_multiple_issues").unwrap();
    let connected_stations = get_unique_stations_connected_by_pathways(&gtfs);
    let stations_and_their_children = get_stations_and_their_children(&gtfs);
    let stations_with_island_stops = get_all_island_stops_in_connected_station(&connected_stations, &stations_and_their_children, 0.06, true);
    assert_eq!(stations_with_island_stops.keys().len(), 2);
    assert_eq!(stations_with_island_stops.get("station_A").unwrap().len(), 1);
    assert_eq!(stations_with_island_stops.get("station_B").unwrap().len(), 1);
    println!("{:?}", stations_with_island_stops);
}


#[test]
fn test_get_platforms_with_boarding_areas(){
    let gtfs = gtfs_structures::Gtfs::new("test_data/pathways/pathways_multiple_issues").unwrap();
    let platforms_with_boarding = get_platforms_with_boarding_areas(&gtfs);
    assert_eq!(platforms_with_boarding.len(), 1);
}

#[test]
fn test_validation_for_pathways_toouching_stations_with_boarding_ares(){
    let gtfs = gtfs_structures::Gtfs::new("test_data/pathways/pathways_multiple_issues").unwrap();
    let connected_stations = get_unique_stations_connected_by_pathways(&gtfs);
    let platforms_with_boarding_areas = get_platforms_with_boarding_areas(&gtfs);
    let issues_with_boarding_areas = get_pathway_touching_platforms_with_boarding_areas_issues_by_station(&connected_stations, &platforms_with_boarding_areas);
    assert_eq!(issues_with_boarding_areas.len(), 1);
}
#[test]
fn test_creation_of_directed_graph(){
    let gtfs = gtfs_structures::Gtfs::new("test_data/pathways/pathways_multiple_issues").unwrap();

    let mut graph = PathwayDirectedGraph::new(0.06, true);
    for stop in gtfs.stops.values() {
        graph.add_stop(&stop);
    }

    let reachable = graph.is_reachable("station_B_concourse","station_A_entrance_1");

    assert!(reachable);
    println!("{}", graph);
}

#[test]
fn test_get_falsely_labelled_accessible_stops(){
    let gtfs = gtfs_structures::Gtfs::new("test_data/pathways/pathways_multiple_issues").unwrap();

    let connected_stations = get_unique_stations_connected_by_pathways(&gtfs);
    let stations_and_their_children = get_stations_and_their_children(&gtfs);
    let falsely_labelled = get_all_falsely_accessible_labelled__stops_in_connected_station(&connected_stations, &stations_and_their_children, 0.06, true);
    assert_eq!(falsely_labelled.keys().len(), 1);

}
#[test]
fn test_pathway_references_invalid_stop(){
    let gtfs = gtfs_structures::Gtfs::new("test_data/pathways/pathways_multiple_issues").unwrap();
    let invalid_stop_ids = get_pathways_with_invalid_stop_references(&gtfs);
    assert_eq!(invalid_stop_ids.len(), 1);
}
#[test]
fn test_levels_read_correctly(){
    let gtfs = gtfs_structures::Gtfs::new("test_data/pathways/pathways_multiple_issues").unwrap();
    let levels = gtfs.levels;
    for level in levels{
        println!("{:#?}", level);
    }

}

#[test]
fn test_stop_access_read_correctly(){
    let gtfs = gtfs_structures::Gtfs::new("test_data/pathways/pathways_multiple_issues").unwrap();
    let stops = gtfs.stops;
    let example_stop = stops.get("station_A_entrance_1").unwrap();
    assert_eq!(example_stop.stop_access,StopAccess::NotDirectlyAccessible)


}

#[test]
fn test_debug(){
    let gtfs = gtfs_structures::Gtfs::new("test_data/pathways/mdb").unwrap();
    let connected_stations = get_unique_stations_connected_by_pathways(&gtfs);
    let stations_and_their_children = get_stations_and_their_children(&gtfs);
    let children = stations_and_their_children.get("de:12065:900203898");

    let platforms_entrances_of_station = get_platforms_and_boarding_areas_and_entrances_for_station(children.unwrap());
    let mut graph_of_station = PathwayDirectedGraph::new(0.06, true);
    for child in children.unwrap() {
        graph_of_station.add_stop(&child);
    }

    for nodes in graph_of_station.adjacent_nodes_wheelchair_accessible{
        println!("{:#?}", nodes);
    }

    for nodes in graph_of_station.adjacent_nodes{
        println!("{:#?}", nodes);
    }


}
