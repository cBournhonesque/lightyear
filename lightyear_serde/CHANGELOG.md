# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## v0.21.0 (2025-07-03)

### Bug Fixes

 - <csr-id-97d5b9baf349aa8c0245d20432ff333c42b2c04d/> issues with prespawn match

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 2 commits contributed to the release.
 - 1 commit was understood as [conventional](https://www.conventionalcommits.org).
 - 1 unique issue was worked on: [#1064](https://github.com/cBournhonesque/lightyear/issues/1064)

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **[#1064](https://github.com/cBournhonesque/lightyear/issues/1064)**
    - Issues with prespawn match ([`97d5b9b`](https://github.com/cBournhonesque/lightyear/commit/97d5b9baf349aa8c0245d20432ff333c42b2c04d))
 * **Uncategorized**
    - Adjusting changelogs prior to release of lightyear_serde v0.21.0, lightyear_utils v0.21.0, lightyear_core v0.21.0, lightyear_link v0.21.0, lightyear_aeronet v0.21.0, lightyear_connection v0.21.0, lightyear_macros v0.21.0, lightyear_transport v0.21.0, lightyear_messages v0.21.0, lightyear_replication v0.21.0, lightyear_sync v0.21.0, lightyear_interpolation v0.21.0, lightyear_prediction v0.21.0, lightyear_frame_interpolation v0.21.0, lightyear_avian2d v0.21.0, lightyear_avian3d v0.21.0, lightyear_crossbeam v0.21.0, lightyear_inputs v0.21.0, lightyear_inputs_bei v0.21.0, lightyear_inputs_leafwing v0.21.0, lightyear_inputs_native v0.21.0, lightyear_netcode v0.21.0, lightyear_steam v0.21.0, lightyear_webtransport v0.21.0, lightyear_udp v0.21.0, lightyear v0.21.0 ([`6ed9ae9`](https://github.com/cBournhonesque/lightyear/commit/6ed9ae95f9a75a9803c75c56c4e81f40f72fc3c8))
</details>

## v0.21.0-rc.3 (2025-07-03)

<csr-id-5dc2e81f8c2b1171df33703d73e38a49e7b4695d/>
<csr-id-81341e91707b31a5cba6967d23e230945180a4e8/>
<csr-id-f9bc3e3d8322d252d80363f716d5e78782520cff/>
<csr-id-b9c22da58aac0aed5d99feb2d3e773582fcf27e4/>
<csr-id-4ae9ac16922d9c160bfb01733a28749a78bfcb3a/>
<csr-id-249b40f358977f6f85e269967d3912bfb4080f73/>

### Chore

 - <csr-id-5dc2e81f8c2b1171df33703d73e38a49e7b4695d/> release rc3
 - <csr-id-81341e91707b31a5cba6967d23e230945180a4e8/> release 0.21 rc 2
 - <csr-id-f9bc3e3d8322d252d80363f716d5e78782520cff/> fix ci
 - <csr-id-b9c22da58aac0aed5d99feb2d3e773582fcf27e4/> fix ci
 - <csr-id-4ae9ac16922d9c160bfb01733a28749a78bfcb3a/> run cargo fmt
 - <csr-id-249b40f358977f6f85e269967d3912bfb4080f73/> fix clippy

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 15 commits contributed to the release.
 - 6 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 6 unique issues were worked on: [#1015](https://github.com/cBournhonesque/lightyear/issues/1015), [#1017](https://github.com/cBournhonesque/lightyear/issues/1017), [#1018](https://github.com/cBournhonesque/lightyear/issues/1018), [#1043](https://github.com/cBournhonesque/lightyear/issues/1043), [#1055](https://github.com/cBournhonesque/lightyear/issues/1055), [#989](https://github.com/cBournhonesque/lightyear/issues/989)

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **[#1015](https://github.com/cBournhonesque/lightyear/issues/1015)**
    - Allow replicating immutable components ([`fb48928`](https://github.com/cBournhonesque/lightyear/commit/fb489288e86fc3438d24f217fe4e82b33909e086))
 * **[#1017](https://github.com/cBournhonesque/lightyear/issues/1017)**
    - Release 0.21 rc1 ([`dc0e61e`](https://github.com/cBournhonesque/lightyear/commit/dc0e61e06fe68309ed8cbfdcdfead633ad567537))
 * **[#1018](https://github.com/cBournhonesque/lightyear/issues/1018)**
    - Separate Connected from LocalId/RemoteId ([`89ce3e7`](https://github.com/cBournhonesque/lightyear/commit/89ce3e705fb262fe819ac1d254468caf3fc5fce5))
 * **[#1043](https://github.com/cBournhonesque/lightyear/issues/1043)**
    - Make workspace crates depend on individual bevy crates ([`5dc3dc3`](https://github.com/cBournhonesque/lightyear/commit/5dc3dc3e17a8b821c35162b904b73eea0e1c69be))
 * **[#1055](https://github.com/cBournhonesque/lightyear/issues/1055)**
    - Release 0.21 rc 2 ([`81341e9`](https://github.com/cBournhonesque/lightyear/commit/81341e91707b31a5cba6967d23e230945180a4e8))
 * **[#989](https://github.com/cBournhonesque/lightyear/issues/989)**
    - Bevy main refactor ([`b236123`](https://github.com/cBournhonesque/lightyear/commit/b236123c8331f9feea8c34cb9e0d6a179bb34918))
 * **Uncategorized**
    - Release lightyear_serde v0.21.0-rc.3, lightyear_utils v0.21.0-rc.3, lightyear_core v0.21.0-rc.3, lightyear_link v0.21.0-rc.3, lightyear_aeronet v0.21.0-rc.3, lightyear_connection v0.21.0-rc.3, lightyear_macros v0.21.0-rc.3, lightyear_transport v0.21.0-rc.3, lightyear_messages v0.21.0-rc.3, lightyear_replication v0.21.0-rc.3, lightyear_sync v0.21.0-rc.3, lightyear_interpolation v0.21.0-rc.3, lightyear_prediction v0.21.0-rc.3, lightyear_frame_interpolation v0.21.0-rc.3, lightyear_avian2d v0.21.0-rc.3, lightyear_avian3d v0.21.0-rc.3, lightyear_crossbeam v0.21.0-rc.3, lightyear_inputs v0.21.0-rc.3, lightyear_inputs_bei v0.21.0-rc.3, lightyear_inputs_leafwing v0.21.0-rc.3, lightyear_inputs_native v0.21.0-rc.3, lightyear_netcode v0.21.0-rc.3, lightyear_steam v0.21.0-rc.3, lightyear_webtransport v0.21.0-rc.3, lightyear_udp v0.21.0-rc.3, lightyear v0.21.0-rc.3 ([`134306e`](https://github.com/cBournhonesque/lightyear/commit/134306eaf4e23d2f609c8a7c93adc3c55618ff11))
    - Release rc3 ([`5dc2e81`](https://github.com/cBournhonesque/lightyear/commit/5dc2e81f8c2b1171df33703d73e38a49e7b4695d))
    - Fix ci ([`f9bc3e3`](https://github.com/cBournhonesque/lightyear/commit/f9bc3e3d8322d252d80363f716d5e78782520cff))
    - Fix ci ([`b9c22da`](https://github.com/cBournhonesque/lightyear/commit/b9c22da58aac0aed5d99feb2d3e773582fcf27e4))
    - Run cargo fmt ([`4ae9ac1`](https://github.com/cBournhonesque/lightyear/commit/4ae9ac16922d9c160bfb01733a28749a78bfcb3a))
    - Fix clippy ([`249b40f`](https://github.com/cBournhonesque/lightyear/commit/249b40f358977f6f85e269967d3912bfb4080f73))
    - Clippy ([`04f11a1`](https://github.com/cBournhonesque/lightyear/commit/04f11a1e1e031ae96f54c29f2803abab32e9a12b))
    - Implement Serializer/Deserializer for LightyearSerde ([`0a7a091`](https://github.com/cBournhonesque/lightyear/commit/0a7a091f734cd2c57d8b4d40b99856d5a13fa32c))
    - Cleanup and move to naia 0.16 ([`15fa3f6`](https://github.com/cBournhonesque/lightyear/commit/15fa3f66bfb279d1f39cc1860bc7ce5ede050787))
</details>

## v0.21.0-rc.2 (2025-06-30)

<csr-id-cedab052a0f47cf91b15267b8d83eb87524a8f4d/>
<csr-id-f9bc3e3d8322d252d80363f716d5e78782520cff/>
<csr-id-b9c22da58aac0aed5d99feb2d3e773582fcf27e4/>
<csr-id-4ae9ac16922d9c160bfb01733a28749a78bfcb3a/>
<csr-id-249b40f358977f6f85e269967d3912bfb4080f73/>

### Chore

 - <csr-id-cedab052a0f47cf91b15267b8d83eb87524a8f4d/> add release command to ci
 - <csr-id-f9bc3e3d8322d252d80363f716d5e78782520cff/> fix ci
 - <csr-id-b9c22da58aac0aed5d99feb2d3e773582fcf27e4/> fix ci
 - <csr-id-4ae9ac16922d9c160bfb01733a28749a78bfcb3a/> run cargo fmt
 - <csr-id-249b40f358977f6f85e269967d3912bfb4080f73/> fix clippy

## v0.21.0-rc.1 (2025-06-08)

<csr-id-f361b72d433086c61ed6b4776fd4ee308c3747e1/>
<csr-id-f241c9deba7c584a345cd2e267a60ab95e0aeb70/>

### Chore

 - <csr-id-f361b72d433086c61ed6b4776fd4ee308c3747e1/> add changelogs

### Chore

 - <csr-id-f241c9deba7c584a345cd2e267a60ab95e0aeb70/> fix std flag

