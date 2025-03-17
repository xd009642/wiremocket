# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
