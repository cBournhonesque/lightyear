# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## v0.21.0-rc.2 (2025-07-01)

<csr-id-cedab052a0f47cf91b15267b8d83eb87524a8f4d/>
<csr-id-72ecbb9604bbb7add8e911cf9d72f21fd00eed6c/>
<csr-id-fe0bb4a24112a308eaf9c829fe5cfae0180ef946/>
<csr-id-249b40f358977f6f85e269967d3912bfb4080f73/>

### Chore

 - <csr-id-cedab052a0f47cf91b15267b8d83eb87524a8f4d/> add release command to ci
 - <csr-id-72ecbb9604bbb7add8e911cf9d72f21fd00eed6c/> add tests for delta-compression
 - <csr-id-fe0bb4a24112a308eaf9c829fe5cfae0180ef946/> fix tests, cargo doc, cargo clippy
 - <csr-id-249b40f358977f6f85e269967d3912bfb4080f73/> fix clippy

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 14 commits contributed to the release over the course of 43 calendar days.
 - 4 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 5 unique issues were worked on: [#1017](https://github.com/cBournhonesque/lightyear/issues/1017), [#1018](https://github.com/cBournhonesque/lightyear/issues/1018), [#1043](https://github.com/cBournhonesque/lightyear/issues/1043), [#1051](https://github.com/cBournhonesque/lightyear/issues/1051), [#989](https://github.com/cBournhonesque/lightyear/issues/989)

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **[#1017](https://github.com/cBournhonesque/lightyear/issues/1017)**
    - Release 0.21 rc1 ([`dc0e61e`](https://github.com/cBournhonesque/lightyear/commit/dc0e61e06fe68309ed8cbfdcdfead633ad567537))
 * **[#1018](https://github.com/cBournhonesque/lightyear/issues/1018)**
    - Separate Connected from LocalId/RemoteId ([`89ce3e7`](https://github.com/cBournhonesque/lightyear/commit/89ce3e705fb262fe819ac1d254468caf3fc5fce5))
 * **[#1043](https://github.com/cBournhonesque/lightyear/issues/1043)**
    - Make workspace crates depend on individual bevy crates ([`5dc3dc3`](https://github.com/cBournhonesque/lightyear/commit/5dc3dc3e17a8b821c35162b904b73eea0e1c69be))
 * **[#1051](https://github.com/cBournhonesque/lightyear/issues/1051)**
    - Add tests for delta-compression ([`72ecbb9`](https://github.com/cBournhonesque/lightyear/commit/72ecbb9604bbb7add8e911cf9d72f21fd00eed6c))
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
    - Fix tests, cargo doc, cargo clippy ([`fe0bb4a`](https://github.com/cBournhonesque/lightyear/commit/fe0bb4a24112a308eaf9c829fe5cfae0180ef946))
    - Fix clippy ([`249b40f`](https://github.com/cBournhonesque/lightyear/commit/249b40f358977f6f85e269967d3912bfb4080f73))
</details>

## v0.21.0-rc.1 (2025-06-08)

<csr-id-f361b72d433086c61ed6b4776fd4ee308c3747e1/>

### Chore

 - <csr-id-f361b72d433086c61ed6b4776fd4ee308c3747e1/> add changelogs

