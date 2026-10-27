# Online tool

transport-validator is the tool used by the
[French National Access Point](https://transport.data.gouv.fr/) to validate GTFS
files. If you want to use it online, you can validate your own files at
[this address](https://transport.data.gouv.fr/validation?locale=en).

**This is a fork of the https://github.com/etalab/transport-validator. This fork eventually intends to become part of the original source.**
This fork supplements the source repository by introducing validations for gtfs pathways, one aspect of the gtfs pathways thats notoriously hard to check.
In doing so, this fork takes an somewhat opinionated approach. It will for example, isssue an warning, if the walkways implicitly connect stop points at two different levels.


## Validations

T `"validations"` key contains the actual validation results.

### Severity

Each check is associated with a severity level.

| Severity    | Description                                                               |
| ----------- | ------------------------------------------------------------------------- |
| Fatal       | Critical error, the GTFS archive couldn't be opened                       |
| Error       | The file does not respect the GTFS specification                          |
| Warning     | Not a specification error, but something is most likely wrong in the data |
| Information | Simple information                                                        |

### List of checks

The fork provides the following validations of pathways. An extensive list can be found at
[issues.rs](https://github.com/etalab/transport-validator/blob/master/src/issues.rs#L21-L83).

Here is a human friendly list of them :

| check name                       | Severity         | Description                                                                                                                                                                              |
| -------------------------------- | ---------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| DistanceBetweenParentAndChildTooHigh | Warning      | Distance between stops exceed a threshold provided by default                                                                                                                            |
| PathwayModeNotCompatibleWithLevels| Error           | Pathway modes Elevator, Stairs and Escalator imply a change in levels. The check is applied when levels.txt is present                                                                   |
|  PathwaySpanTooHigh              | Error            | This distance between two stops connected by the pathway exceed an user provided threshold                                                                                               |
|  NoCommonAncestor                | Error            |  A pathway should only connect points that lie within the same station                                                                                                                   |
| DanglingStop                   | Error           |  Pathways imply that a given stop is an island (not connected to any other stop point )                                                                                                  |
| LockedPlatform                   | Error            |  A platform (stop with LocationType=0) is not reachable from any exit/entrance                                                                                                           |
| TransitiveIncompabilityOfPathways | Warning         |  Incompatible pathway modes implied by two more more pair of pathways (see https://github.com/google/transit/issues/661)                                                                 |
| InconsitentWheelchairAccessbility | Warning          |  Wheelchair accessibilty label for the stop inconsistent with the information implied by pathways                                                                                        | 
| LockedPlatform                     |Error           | If a station is connected by pathways, every platform and boarding area must be reachable from at least one exit/entrance |
|PathwayToPlatformWithBoardingAreas | Error | A pathway is connecting stops which have boarding areas |
|PathwayHasInvalidStopId | Error | A pathway references stop_id's that are invalid |

The validation produces two additional artefacts
1. stations_and_issues.json collects the pathway related issues by station
2. station_ranking.json is a list of stations along with their accessibility score. This is a simple index caluclated in the following way: It just represents the fraction of platforms reachable by a chain of wheelchair accessible pathways as a fraction of all platforms.

These two files are consumed by simple visualizer.html 
## Run the validator
The easiest way to run the validator is to carry out the validation of an unzipped gtfs feed on file.
The following steps are optimal
```bash
cargo build --release 
run_and_visualize.sh <path_to_gtfs_feed>
```

This will launch the visualizer at port 8999. Please note that visualizer is not a core functionality 
of the fork. The main restriction in making it a core functionality is the availability of a truly open
vector tile service that covers the entire world.

### Benchmarking

Validating the Verkehrsverbund Berlin-Brandenburg GTFS Feed (https://mobilitydatabase.org/feeds/gtfs/mdb-3233) with 42000 stops and over 120,000 pathways
takes about 13 seconds on M1 Macbook with 8GB RAM.

