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
<csr-id-72ecbb9604bbb7add8e911cf9d72f21fd00eed6c/>
<csr-id-f9bc3e3d8322d252d80363f716d5e78782520cff/>
<csr-id-b9c22da58aac0aed5d99feb2d3e773582fcf27e4/>
<csr-id-4ae9ac16922d9c160bfb01733a28749a78bfcb3a/>
<csr-id-249b40f358977f6f85e269967d3912bfb4080f73/>
<csr-id-f55c117c1627368978d26c788efbcb2ddda1da01/>
<csr-id-bc7cf371f822ff7a2667c329b6f77e5a694a93d4/>

### Chore

 - <csr-id-5dc2e81f8c2b1171df33703d73e38a49e7b4695d/> release rc3
 - <csr-id-81341e91707b31a5cba6967d23e230945180a4e8/> release 0.21 rc 2
 - <csr-id-72ecbb9604bbb7add8e911cf9d72f21fd00eed6c/> add tests for delta-compression
 - <csr-id-f9bc3e3d8322d252d80363f716d5e78782520cff/> fix ci
 - <csr-id-b9c22da58aac0aed5d99feb2d3e773582fcf27e4/> fix ci
 - <csr-id-4ae9ac16922d9c160bfb01733a28749a78bfcb3a/> run cargo fmt
 - <csr-id-249b40f358977f6f85e269967d3912bfb4080f73/> fix clippy
 - <csr-id-f55c117c1627368978d26c788efbcb2ddda1da01/> cargo fmt
 - <csr-id-bc7cf371f822ff7a2667c329b6f77e5a694a93d4/> enable host-server for all examples

### Bug Fixes

 - <csr-id-e85935036975bb7bda4f2d77fb00df66084cc513/> fix bug on fps example with missing PlayerMarker component

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 16 commits contributed to the release over the course of 45 calendar days.
 - 10 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 9 unique issues were worked on: [#1017](https://github.com/cBournhonesque/lightyear/issues/1017), [#1023](https://github.com/cBournhonesque/lightyear/issues/1023), [#1029](https://github.com/cBournhonesque/lightyear/issues/1029), [#1043](https://github.com/cBournhonesque/lightyear/issues/1043), [#1047](https://github.com/cBournhonesque/lightyear/issues/1047), [#1049](https://github.com/cBournhonesque/lightyear/issues/1049), [#1051](https://github.com/cBournhonesque/lightyear/issues/1051), [#1055](https://github.com/cBournhonesque/lightyear/issues/1055), [#989](https://github.com/cBournhonesque/lightyear/issues/989)

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **[#1017](https://github.com/cBournhonesque/lightyear/issues/1017)**
    - Release 0.21 rc1 ([`dc0e61e`](https://github.com/cBournhonesque/lightyear/commit/dc0e61e06fe68309ed8cbfdcdfead633ad567537))
 * **[#1023](https://github.com/cBournhonesque/lightyear/issues/1023)**
    - Add HostServer ([`5b6af7e`](https://github.com/cBournhonesque/lightyear/commit/5b6af7edd3b41c05333d14dde258ea5e89c07c2d))
 * **[#1029](https://github.com/cBournhonesque/lightyear/issues/1029)**
    - Enable host-server for all examples ([`bc7cf37`](https://github.com/cBournhonesque/lightyear/commit/bc7cf371f822ff7a2667c329b6f77e5a694a93d4))
 * **[#1043](https://github.com/cBournhonesque/lightyear/issues/1043)**
    - Make workspace crates depend on individual bevy crates ([`5dc3dc3`](https://github.com/cBournhonesque/lightyear/commit/5dc3dc3e17a8b821c35162b904b73eea0e1c69be))
 * **[#1047](https://github.com/cBournhonesque/lightyear/issues/1047)**
    - Fix bug on fps example with missing PlayerMarker component ([`e859350`](https://github.com/cBournhonesque/lightyear/commit/e85935036975bb7bda4f2d77fb00df66084cc513))
 * **[#1049](https://github.com/cBournhonesque/lightyear/issues/1049)**
    - Alternative replication system + fix delta-compression ([`4d5e690`](https://github.com/cBournhonesque/lightyear/commit/4d5e69072485faa3975543792a8e11be7608a0ea))
 * **[#1051](https://github.com/cBournhonesque/lightyear/issues/1051)**
    - Add tests for delta-compression ([`72ecbb9`](https://github.com/cBournhonesque/lightyear/commit/72ecbb9604bbb7add8e911cf9d72f21fd00eed6c))
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
    - Cargo fmt ([`f55c117`](https://github.com/cBournhonesque/lightyear/commit/f55c117c1627368978d26c788efbcb2ddda1da01))
</details>

## v0.21.0-rc.2 (2025-07-01)

<csr-id-cedab052a0f47cf91b15267b8d83eb87524a8f4d/>
<csr-id-72ecbb9604bbb7add8e911cf9d72f21fd00eed6c/>
<csr-id-f9bc3e3d8322d252d80363f716d5e78782520cff/>
<csr-id-b9c22da58aac0aed5d99feb2d3e773582fcf27e4/>
<csr-id-4ae9ac16922d9c160bfb01733a28749a78bfcb3a/>
<csr-id-249b40f358977f6f85e269967d3912bfb4080f73/>
<csr-id-f55c117c1627368978d26c788efbcb2ddda1da01/>
<csr-id-bc7cf371f822ff7a2667c329b6f77e5a694a93d4/>

### Chore

 - <csr-id-cedab052a0f47cf91b15267b8d83eb87524a8f4d/> add release command to ci
 - <csr-id-72ecbb9604bbb7add8e911cf9d72f21fd00eed6c/> add tests for delta-compression
 - <csr-id-f9bc3e3d8322d252d80363f716d5e78782520cff/> fix ci
 - <csr-id-b9c22da58aac0aed5d99feb2d3e773582fcf27e4/> fix ci
 - <csr-id-4ae9ac16922d9c160bfb01733a28749a78bfcb3a/> run cargo fmt
 - <csr-id-249b40f358977f6f85e269967d3912bfb4080f73/> fix clippy
 - <csr-id-f55c117c1627368978d26c788efbcb2ddda1da01/> cargo fmt
 - <csr-id-bc7cf371f822ff7a2667c329b6f77e5a694a93d4/> enable host-server for all examples

### Bug Fixes

 - <csr-id-e85935036975bb7bda4f2d77fb00df66084cc513/> fix bug on fps example with missing PlayerMarker component

## v0.21.0-rc.1 (2025-06-08)

<csr-id-f361b72d433086c61ed6b4776fd4ee308c3747e1/>

### Chore

 - <csr-id-f361b72d433086c61ed6b4776fd4ee308c3747e1/> add changelogs

