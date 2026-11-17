# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- The web desktop asks you to log on, and Log Off returns to the logon box. Accounts that are not administrators can read but cannot change the server (#102)

### Changed

- NT4 domain and network services now run from published `lanlord-server-component` images (`nt4` with Samba built from source, `network-services` with dnsmasq). Pin a Samba version with `nt4-domain.samba_version`. The old `image` and `build_locally` settings are gone.
