use crate::{custom_rules, Issue, IssueType, Severity};
use gtfs_structures::{Availability, LocationType, Pathway, PathwayDirectionType, PathwayMode, Stop};
use std::collections::HashMap;
use std::sync::Arc;
use rayon::prelude::*;
use geo::{Distance as _, Haversine, Point};
use crate::custom_rules::{custom_rules, CustomRules};

pub fn validate(gtfs: &gtfs_structures::Gtfs,custom_rules: &CustomRules) -> Vec<Issue> {
    validate_distance_spanned_by_pathway(gtfs,custom_rules)
        .into_iter()
        .chain(validate_ancestor_of_pathways(gtfs))
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