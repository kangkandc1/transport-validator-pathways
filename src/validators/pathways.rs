use crate::{Issue, IssueType, Severity};
use gtfs_structures::{Availability, Gtfs, LocationType, Pathway, PathwayDirectionType, PathwayMode, Stop};
use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::sync::Arc;
use clap::builder::Str;
use rayon::prelude::*;
use geo::{Distance as _, Haversine, Point};
use crate::custom_rules::{custom_rules, CustomRules};

pub fn validate(gtfs: &gtfs_structures::Gtfs,custom_rules: &CustomRules) -> Vec<Issue> {
    validate_distance_spanned_by_pathway(gtfs,custom_rules)
        .into_iter()
        .chain(validate_ancestor_of_pathways(gtfs))
        .into_iter()
        .chain(validate_pathways_has_compatible_levels(gtfs))
        .into_iter()
        .chain(validate_no_dangling_stops(gtfs))
        .into_iter()
        .chain(validate_pathway_doesnt_touch_plaftforms_with_boarding_areas(gtfs))
        .collect()
}

fn validate_ancestor_of_pathways(gtfs: &gtfs_structures::Gtfs) -> Vec<Issue> {
    gtfs.stops
        .values()
        .collect::<Vec<_>>()          // collect into Vec for par_iter
        .par_iter()                    // parallel iterator over &Arc<Stop>
        .flat_map_iter(|stop_arc| stop_arc.as_ref().pathways.iter())
        .filter(|pathway| !pathway_connecting_stops_with_same_ancestor(pathway, &gtfs.stops))
        .map(make_no_common_ancestor_for_pathway_issue)
        .collect()
}

fn validate_distance_spanned_by_pathway(gtfs: &gtfs_structures::Gtfs,custom_rules: &CustomRules) -> Vec<Issue> {

    let Some(threshold) = custom_rules.max_distance_spanned_by_pathway else {
        return Vec::new();
    };

    gtfs.stops
        .values()
        .collect::<Vec<_>>()
        .par_iter()
        .flat_map_iter(|stop_arc| stop_arc.as_ref().pathways.iter())
        .filter(|pathway| {
            pathway_spanning_distance_above_threshold(pathway, &gtfs.stops, threshold)
        })
        .map(|pathway| make_pathways_connecting_stops_too_far_issue(pathway, &threshold))
        .collect()
}

fn validate_pathways_has_compatible_levels(gtfs: &gtfs_structures::Gtfs) -> Vec<Issue> {
    gtfs.stops
        .values()
        .collect::<Vec<_>>()          // collect into Vec for par_iter
        .par_iter()                    // parallel iterator over &Arc<Stop>
        .flat_map_iter(|stop_arc| stop_arc.as_ref().pathways.iter())
        .filter(|pathway| !pathway_has_compatible_levels(pathway, &gtfs.stops))
        .map(make_pathway_doesnt_have_compatible_level_issue)
        .collect()
}

fn validate_no_dangling_stops(gtfs: &gtfs_structures::Gtfs) -> Vec<Issue> {
    let mut issues: Vec<Issue> = Vec::new();
    let dangling_stops = get_all_stops_in_connected_station_not_connected_by_pathways(gtfs);
    if dangling_stops.is_empty() {
        return Vec::new();
    }

    for stop in dangling_stops {
        let parent_stop = stop.0;
        let dangling_stops = stop.1;
        for dangling_stop in dangling_stops {
            let new_dangling_issue = make_dangling_stop_issue(parent_stop.clone(),dangling_stop.id.clone());
            issues.push(new_dangling_issue);
        }
    }

    issues

}

fn validate_pathway_doesnt_touch_plaftforms_with_boarding_areas(
    gtfs: &gtfs_structures::Gtfs,
) -> Vec<Issue> {
    let mut issues: Vec<Issue> = Vec::new();
    let platforms_with_boarding_areas = get_platforms_with_boarding_areas(gtfs);

    gtfs.stops
        .values()
        .collect::<Vec<_>>()
        .par_iter()
        .flat_map_iter(|stop_arc| stop_arc.as_ref().pathways.iter())
        .filter_map(|pathway| {
            pathway_connected_to_platform_with_boarding_areas(
                pathway,
                &platforms_with_boarding_areas,   // note the & — signature takes a reference
            )
                .map(|reason| (pathway, reason))      // Some((pathway, reason)) or None
        })
        .map(|(pathway, reason)| {
            make_pathway_to_platform_with_boarding_areas_issue(pathway, reason)
        })
        .collect()
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
        | LocationType::GenericNode => stop
            .parent_station
            .as_deref()
            .and_then(|p| all_stops.get(p))
            .cloned(),

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
    gtfs: &Gtfs,
) -> HashMap<String, Vec<Arc<Stop>>> {
    let connected_stations = get_unique_stations_connected_by_pathways(gtfs);
    let stations_and_their_children = get_stations_and_their_children(gtfs);

    let mut dangling_stops: HashMap<String, Vec<Arc<Stop>>> = HashMap::new();

    for (station_id, connected_children) in &connected_stations {
        let connected_ids: HashSet<&str> = connected_children
            .iter()
            .map(|s| s.id.as_str())
            .collect();

        let Some(children) = stations_and_their_children.get(station_id) else {
            continue;
        };

        let missing: Vec<Arc<Stop>> = children
            .iter()
            .filter(|s| s.location_type != LocationType::BoardingArea && !connected_ids.contains(s.id.as_str()))
            .cloned()
            .collect();

        if !missing.is_empty() {
            dangling_stops.insert(station_id.clone(), missing);
        }
    }


    dangling_stops
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
    if(is_from_id_invalid){
         return Some(from_id.clone());
    };
    if(is_to_id_invalid){
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
            Haversine.distance(a,b)>threshold
        }
        _ => false,
    }
}

fn make_no_common_ancestor_for_pathway_issue(pathway: &Pathway) -> Issue {
    let base_issue = Issue::new(Severity::Error, IssueType::PathwayIncompatibleAncestor, &pathway.id);
    let message = format!("the pathway with id {}   connects stops {} to stop {} having different ancestor ", pathway.id, pathway.from_stop_id, pathway.to_stop_id);
    base_issue.details(message.as_str())
}

fn make_pathways_connecting_stops_too_far_issue(pathway: &gtfs_structures::Pathway,theshold:&f64) -> Issue {
    let base_issue = Issue::new(Severity::Error,IssueType::PathwayConnectingStopsTooFar,&pathway.id);
    let message = format!("the pathway with id {}   connects stops {} to stop {} spans a distance more than the user provided threshold {} meters", pathway.id, pathway.from_stop_id, pathway.to_stop_id,theshold);

    base_issue.details(message.as_str())

}


fn make_pathway_doesnt_have_compatible_level_issue(pathway: &Pathway)->Issue {
    let base_issue = Issue::new(Severity::Error, IssueType::PathwayModeNotCompatibleWithLevels, &pathway.id);
    let mode = format!("Pathway mode is {:?}", pathway.mode);
    let message = format!("the pathway with id {}   connects stops {} to stop {} is incompatible with mode {} ", pathway.id, pathway.from_stop_id, pathway.to_stop_id,mode);
    base_issue.details(message.as_str())
}

fn make_dangling_stop_issue(parent_stop_id:String,dangling_stop_id:String)->Issue {
    let base_issue = Issue::new(Severity::Error,IssueType::DanglingStop,&dangling_stop_id);
    let message = format!("Station with id {} has a dangling stop with id {}", parent_stop_id, dangling_stop_id);
    base_issue.details(message.as_str())
}

fn make_pathway_to_platform_with_boarding_areas_issue(pathway: &gtfs_structures::Pathway,problem_station_id:String)->Issue {
    let base_issue= Issue::new(Severity::Error,IssueType::PathwayToPlatformWithBoardingAreas,&problem_station_id);
    let message = format!("pathway with id {} is connected to location with id {} that contains boarding areas",pathway.id, problem_station_id);
    base_issue.details(message.as_str())
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
    assert_eq!(issues.get(0).unwrap().object_id,"pw_broken");
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
    let issues = validate_distance_spanned_by_pathway(&gtfs,&custom_rules);

    println!("issues: {:?}", issues);
    assert!(issues.len()>0);
}

#[test]
fn test_validating_compatible_modes_creates_issue(){
    let gtfs = gtfs_structures::Gtfs::new("test_data/pathways/pathways_multiple_issues").unwrap();
    let file_path = Some(String::from("test_data/custom_rules/custom_rules.yml"));
    let custom_rules = custom_rules(file_path);
    let issues = validate_pathways_has_compatible_levels(&gtfs);

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
    assert_eq!(station_children_map.get("station_A").unwrap().len(),10);
    assert_eq!(station_children_map.get("station_B").unwrap().len(),5);
}

#[test]
fn test_get_map_of_stations_with_dangling_stops(){
    let gtfs = gtfs_structures::Gtfs::new("test_data/pathways/pathways_multiple_issues").unwrap();
    let stations_with_dangling_stops = get_all_stops_in_connected_station_not_connected_by_pathways(&gtfs);
    assert_eq!(stations_with_dangling_stops.keys().len(),1);
    assert_eq!(stations_with_dangling_stops.get("station_A").unwrap().len(),1);
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
    let issues_with_boarding_areas = validate_pathway_doesnt_touch_plaftforms_with_boarding_areas(&gtfs);
    assert_eq!(issues_with_boarding_areas.len(), 1);
}