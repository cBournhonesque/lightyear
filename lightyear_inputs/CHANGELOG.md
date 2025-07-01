# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## v0.21.0-rc.2 (2025-07-01)

<csr-id-cedab052a0f47cf91b15267b8d83eb87524a8f4d/>
<csr-id-f55c117c1627368978d26c788efbcb2ddda1da01/>
<csr-id-bc7cf371f822ff7a2667c329b6f77e5a694a93d4/>
<csr-id-c6f10d8162db1cb1b2c9f9d3f8f2be2cb181102b/>

### Chore

 - <csr-id-cedab052a0f47cf91b15267b8d83eb87524a8f4d/> add release command to ci
 - <csr-id-f55c117c1627368978d26c788efbcb2ddda1da01/> cargo fmt
 - <csr-id-bc7cf371f822ff7a2667c329b6f77e5a694a93d4/> enable host-server for all examples

### New Features

 - <csr-id-117b0841a25dba5c6ffaadad88a8c4dba09d3cbb/> support BEI inputs

### Other

 - <csr-id-c6f10d8162db1cb1b2c9f9d3f8f2be2cb181102b/> fix inputs

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 18 commits contributed to the release over the course of 43 calendar days.
 - 5 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 9 unique issues were worked on: [#1017](https://github.com/cBournhonesque/lightyear/issues/1017), [#1018](https://github.com/cBournhonesque/lightyear/issues/1018), [#1023](https://github.com/cBournhonesque/lightyear/issues/1023), [#1029](https://github.com/cBournhonesque/lightyear/issues/1029), [#1039](https://github.com/cBournhonesque/lightyear/issues/1039), [#1040](https://github.com/cBournhonesque/lightyear/issues/1040), [#1043](https://github.com/cBournhonesque/lightyear/issues/1043), [#1049](https://github.com/cBournhonesque/lightyear/issues/1049), [#989](https://github.com/cBournhonesque/lightyear/issues/989)

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **[#1017](https://github.com/cBournhonesque/lightyear/issues/1017)**
    - Release 0.21 rc1 ([`dc0e61e`](https://github.com/cBournhonesque/lightyear/commit/dc0e61e06fe68309ed8cbfdcdfead633ad567537))
 * **[#1018](https://github.com/cBournhonesque/lightyear/issues/1018)**
    - Separate Connected from LocalId/RemoteId ([`89ce3e7`](https://github.com/cBournhonesque/lightyear/commit/89ce3e705fb262fe819ac1d254468caf3fc5fce5))
 * **[#1023](https://github.com/cBournhonesque/lightyear/issues/1023)**
    - Add HostServer ([`5b6af7e`](https://github.com/cBournhonesque/lightyear/commit/5b6af7edd3b41c05333d14dde258ea5e89c07c2d))
 * **[#1029](https://github.com/cBournhonesque/lightyear/issues/1029)**
    - Enable host-server for all examples ([`bc7cf37`](https://github.com/cBournhonesque/lightyear/commit/bc7cf371f822ff7a2667c329b6f77e5a694a93d4))
 * **[#1039](https://github.com/cBournhonesque/lightyear/issues/1039)**
    - Support BEI inputs ([`117b084`](https://github.com/cBournhonesque/lightyear/commit/117b0841a25dba5c6ffaadad88a8c4dba09d3cbb))
 * **[#1040](https://github.com/cBournhonesque/lightyear/issues/1040)**
    - Fix inputs ([`c6f10d8`](https://github.com/cBournhonesque/lightyear/commit/c6f10d8162db1cb1b2c9f9d3f8f2be2cb181102b))
 * **[#1043](https://github.com/cBournhonesque/lightyear/issues/1043)**
    - Make workspace crates depend on individual bevy crates ([`5dc3dc3`](https://github.com/cBournhonesque/lightyear/commit/5dc3dc3e17a8b821c35162b904b73eea0e1c69be))
 * **[#1049](https://github.com/cBournhonesque/lightyear/issues/1049)**
    - Alternative replication system + fix delta-compression ([`4d5e690`](https://github.com/cBournhonesque/lightyear/commit/4d5e69072485faa3975543792a8e11be7608a0ea))
 * **[#989](https://github.com/cBournhonesque/lightyear/issues/989)**
    - Bevy main refactor ([`b236123`](https://github.com/cBournhonesque/lightyear/commit/b236123c8331f9feea8c34cb9e0d6a179bb34918))
 * **Uncategorized**
    - Release lightyear_replication v0.21.0-rc.2, lightyear_sync v0.21.0-rc.2, lightyear_interpolation v0.21.0-rc.2, lightyear_prediction v0.21.0-rc.2, lightyear_frame_interpolation v0.21.0-rc.2, lightyear_avian v0.21.0-rc.2, lightyear_crossbeam v0.21.0-rc.2, lightyear_inputs v0.21.0-rc.2, lightyear_inputs_bei v0.21.0-rc.2, lightyear_inputs_leafwing v0.21.0-rc.2, lightyear_inputs_native v0.21.0-rc.2, lightyear_netcode v0.21.0-rc.2, lightyear_steam v0.21.0-rc.2, lightyear_webtransport v0.21.0-rc.2, lightyear_udp v0.21.0-rc.2, lightyear v0.21.0-rc.2 ([`b6dc58a`](https://github.com/cBournhonesque/lightyear/commit/b6dc58ac14426fb5ed211fc07af46be137a3cb34))
    - Release lightyear_messages v0.21.0-rc.2, lightyear_replication v0.21.0-rc.2, lightyear_sync v0.21.0-rc.2, lightyear_interpolation v0.21.0-rc.2, lightyear_prediction v0.21.0-rc.2, lightyear_frame_interpolation v0.21.0-rc.2, lightyear_avian v0.21.0-rc.2, lightyear_crossbeam v0.21.0-rc.2, lightyear_inputs v0.21.0-rc.2, lightyear_inputs_bei v0.21.0-rc.2, lightyear_inputs_leafwing v0.21.0-rc.2, lightyear_inputs_native v0.21.0-rc.2, lightyear_netcode v0.21.0-rc.2, lightyear_steam v0.21.0-rc.2, lightyear_webtransport v0.21.0-rc.2, lightyear_udp v0.21.0-rc.2, lightyear v0.21.0-rc.2 ([`156d504`](https://github.com/cBournhonesque/lightyear/commit/156d5044566e38244b1761401e799f33f84009bb))
    - Release lightyear_transport v0.21.0-rc.2, lightyear_messages v0.21.0-rc.2, lightyear_replication v0.21.0-rc.2, lightyear_sync v0.21.0-rc.2, lightyear_interpolation v0.21.0-rc.2, lightyear_prediction v0.21.0-rc.2, lightyear_frame_interpolation v0.21.0-rc.2, lightyear_avian v0.21.0-rc.2, lightyear_crossbeam v0.21.0-rc.2, lightyear_inputs v0.21.0-rc.2, lightyear_inputs_bei v0.21.0-rc.2, lightyear_inputs_leafwing v0.21.0-rc.2, lightyear_inputs_native v0.21.0-rc.2, lightyear_netcode v0.21.0-rc.2, lightyear_steam v0.21.0-rc.2, lightyear_webtransport v0.21.0-rc.2, lightyear_udp v0.21.0-rc.2, lightyear v0.21.0-rc.2 ([`a52b3b8`](https://github.com/cBournhonesque/lightyear/commit/a52b3b89dcbdf7dc99d55255c37bb1197f906abd))
    - Release lightyear_transport v0.21.0-rc.2, lightyear_messages v0.21.0-rc.2, lightyear_replication v0.21.0-rc.2, lightyear_sync v0.21.0-rc.2, lightyear_interpolation v0.21.0-rc.2, lightyear_prediction v0.21.0-rc.2, lightyear_frame_interpolation v0.21.0-rc.2, lightyear_avian v0.21.0-rc.2, lightyear_crossbeam v0.21.0-rc.2, lightyear_inputs v0.21.0-rc.2, lightyear_inputs_bei v0.21.0-rc.2, lightyear_inputs_leafwing v0.21.0-rc.2, lightyear_inputs_native v0.21.0-rc.2, lightyear_netcode v0.21.0-rc.2, lightyear_steam v0.21.0-rc.2, lightyear_webtransport v0.21.0-rc.2, lightyear_udp v0.21.0-rc.2, lightyear v0.21.0-rc.2 ([`af910bc`](https://github.com/cBournhonesque/lightyear/commit/af910bc2c162ec521b55003610a54023f6c034ce))
    - Release lightyear_transport v0.21.0-rc.2, lightyear_messages v0.21.0-rc.2, lightyear_replication v0.21.0-rc.2, lightyear_sync v0.21.0-rc.2, lightyear_interpolation v0.21.0-rc.2, lightyear_prediction v0.21.0-rc.2, lightyear_frame_interpolation v0.21.0-rc.2, lightyear_avian v0.21.0-rc.2, lightyear_crossbeam v0.21.0-rc.2, lightyear_inputs v0.21.0-rc.2, lightyear_inputs_bei v0.21.0-rc.2, lightyear_inputs_leafwing v0.21.0-rc.2, lightyear_inputs_native v0.21.0-rc.2, lightyear_netcode v0.21.0-rc.2, lightyear_steam v0.21.0-rc.2, lightyear_webtransport v0.21.0-rc.2, lightyear_udp v0.21.0-rc.2, lightyear v0.21.0-rc.2 ([`244077f`](https://github.com/cBournhonesque/lightyear/commit/244077f9e729f0c267e6b865c244ac915f6d244f))
    - Release lightyear_serde v0.21.0-rc.2, lightyear_utils v0.21.0-rc.2, lightyear_core v0.21.0-rc.2, lightyear_link v0.21.0-rc.2, lightyear_aeronet v0.21.0-rc.2, lightyear_connection v0.21.0-rc.2, lightyear_macros v0.21.0-rc.2, lightyear_transport v0.21.0-rc.2, lightyear_messages v0.21.0-rc.2, lightyear_replication v0.21.0-rc.2, lightyear_sync v0.21.0-rc.2, lightyear_interpolation v0.21.0-rc.2, lightyear_prediction v0.21.0-rc.2, lightyear_frame_interpolation v0.21.0-rc.2, lightyear_avian v0.21.0-rc.2, lightyear_crossbeam v0.21.0-rc.2, lightyear_inputs v0.21.0-rc.2, lightyear_inputs_bei v0.21.0-rc.2, lightyear_inputs_leafwing v0.21.0-rc.2, lightyear_inputs_native v0.21.0-rc.2, lightyear_netcode v0.21.0-rc.2, lightyear_steam v0.21.0-rc.2, lightyear_webtransport v0.21.0-rc.2, lightyear_udp v0.21.0-rc.2, lightyear v0.21.0-rc.2 ([`89f1549`](https://github.com/cBournhonesque/lightyear/commit/89f1549f6d9e79719561dadaa8ff1f8d6772f77d))
    - Add release command to ci ([`cedab05`](https://github.com/cBournhonesque/lightyear/commit/cedab052a0f47cf91b15267b8d83eb87524a8f4d))
    - Cargo fmt ([`f55c117`](https://github.com/cBournhonesque/lightyear/commit/f55c117c1627368978d26c788efbcb2ddda1da01))
    - Fix lints ([`9040874`](https://github.com/cBournhonesque/lightyear/commit/904087429078e4bbda90a01edd0a0bad68801767))
</details>

## v0.21.0-rc.1 (2025-06-08)

<csr-id-f361b72d433086c61ed6b4776fd4ee308c3747e1/>

### Chore

 - <csr-id-f361b72d433086c61ed6b4776fd4ee308c3747e1/> add changelogs

