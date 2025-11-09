# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.1] - 2024-11-09
### Changed
- Update tungstenite to 0.28.0

## [0.3.0] - 2025-06-22
### Changed
- Renamed `one_to_one_response` to more intuitive `response_map` 

## [0.2.0] - 2025-03-18
### Added
- Builder for the `MockServer`
- Ability to create a `MockServer` with a fixed IP.

### Changed
- Mock creation now goes via a builder type to prevent some correctness issues involving cloning and
attaching multiple modified copies of a mock
- Lambda function for matcher now has to return an `Option<bool>`
- Fixed `HeaderExactMatcher` to actually match headers exactly

### Fixed
- Fixed some intra-doc links which were broken

## [0.1.0] - 2025-03-03

### Added
- Added mock server
- Matching for requests (path/query parameters/headers)
- Matching for stream contents
- Responder abstraction
