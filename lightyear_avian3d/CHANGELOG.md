# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## v0.21.0-rc.2 (2025-07-01)

<csr-id-cedab052a0f47cf91b15267b8d83eb87524a8f4d/>
<csr-id-4ae9ac16922d9c160bfb01733a28749a78bfcb3a/>
<csr-id-249b40f358977f6f85e269967d3912bfb4080f73/>
<csr-id-f55c117c1627368978d26c788efbcb2ddda1da01/>
<csr-id-bc7cf371f822ff7a2667c329b6f77e5a694a93d4/>

### Chore

 - <csr-id-cedab052a0f47cf91b15267b8d83eb87524a8f4d/> add release command to ci
 - <csr-id-4ae9ac16922d9c160bfb01733a28749a78bfcb3a/> run cargo fmt
 - <csr-id-249b40f358977f6f85e269967d3912bfb4080f73/> fix clippy
 - <csr-id-f55c117c1627368978d26c788efbcb2ddda1da01/> cargo fmt
 - <csr-id-bc7cf371f822ff7a2667c329b6f77e5a694a93d4/> enable host-server for all examples

### Bug Fixes

 - <csr-id-e85935036975bb7bda4f2d77fb00df66084cc513/> fix bug on fps example with missing PlayerMarker component
 - <csr-id-1108da74e019d8efc37728b58ab07ac9472aaefa/> fix bug on fps example with missing PlayerMarker component

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 24 commits contributed to the release.
 - 7 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 12 unique issues were worked on: [#1017](https://github.com/cBournhonesque/lightyear/issues/1017), [#1018](https://github.com/cBournhonesque/lightyear/issues/1018), [#1029](https://github.com/cBournhonesque/lightyear/issues/1029), [#1043](https://github.com/cBournhonesque/lightyear/issues/1043), [#1047](https://github.com/cBournhonesque/lightyear/issues/1047), [#1054](https://github.com/cBournhonesque/lightyear/issues/1054), [#849](https://github.com/cBournhonesque/lightyear/issues/849), [#853](https://github.com/cBournhonesque/lightyear/issues/853), [#965](https://github.com/cBournhonesque/lightyear/issues/965), [#966](https://github.com/cBournhonesque/lightyear/issues/966), [#989](https://github.com/cBournhonesque/lightyear/issues/989), [#999](https://github.com/cBournhonesque/lightyear/issues/999)

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **[#1017](https://github.com/cBournhonesque/lightyear/issues/1017)**
    - Release 0.21 rc1 ([`dc0e61e`](https://github.com/cBournhonesque/lightyear/commit/dc0e61e06fe68309ed8cbfdcdfead633ad567537))
 * **[#1018](https://github.com/cBournhonesque/lightyear/issues/1018)**
    - Separate Connected from LocalId/RemoteId ([`89ce3e7`](https://github.com/cBournhonesque/lightyear/commit/89ce3e705fb262fe819ac1d254468caf3fc5fce5))
 * **[#1029](https://github.com/cBournhonesque/lightyear/issues/1029)**
    - Enable host-server for all examples ([`bc7cf37`](https://github.com/cBournhonesque/lightyear/commit/bc7cf371f822ff7a2667c329b6f77e5a694a93d4))
 * **[#1043](https://github.com/cBournhonesque/lightyear/issues/1043)**
    - Make workspace crates depend on individual bevy crates ([`5dc3dc3`](https://github.com/cBournhonesque/lightyear/commit/5dc3dc3e17a8b821c35162b904b73eea0e1c69be))
 * **[#1047](https://github.com/cBournhonesque/lightyear/issues/1047)**
    - Fix bug on fps example with missing PlayerMarker component ([`e859350`](https://github.com/cBournhonesque/lightyear/commit/e85935036975bb7bda4f2d77fb00df66084cc513))
    - Fix bug on fps example with missing PlayerMarker component ([`1108da7`](https://github.com/cBournhonesque/lightyear/commit/1108da74e019d8efc37728b58ab07ac9472aaefa))
 * **[#1054](https://github.com/cBournhonesque/lightyear/issues/1054)**
    - Chore(docs) ([`59b9f7e`](https://github.com/cBournhonesque/lightyear/commit/59b9f7eb37b036488d3ceab780074274074a9bd6))
 * **[#849](https://github.com/cBournhonesque/lightyear/issues/849)**
    - Add lag compensation ([`db78764`](https://github.com/cBournhonesque/lightyear/commit/db78764289dd410eea511c516be55772c1970e23))
 * **[#853](https://github.com/cBournhonesque/lightyear/issues/853)**
    - 0.19 ([`e61e01a`](https://github.com/cBournhonesque/lightyear/commit/e61e01a96417164de1eddeee7d52ca6cb6c497af))
 * **[#965](https://github.com/cBournhonesque/lightyear/issues/965)**
    - Change logs to info ([`2037d46`](https://github.com/cBournhonesque/lightyear/commit/2037d468f513569deee79ca24e0eb06c2a4c35ea))
    - Change logs to info ([`76c52c0`](https://github.com/cBournhonesque/lightyear/commit/76c52c0d61ee02baba89f4f2129a7d0f4200cb1d))
 * **[#966](https://github.com/cBournhonesque/lightyear/issues/966)**
    - Change logs to info ([`2037d46`](https://github.com/cBournhonesque/lightyear/commit/2037d468f513569deee79ca24e0eb06c2a4c35ea))
 * **[#989](https://github.com/cBournhonesque/lightyear/issues/989)**
    - Bevy main refactor ([`b236123`](https://github.com/cBournhonesque/lightyear/commit/b236123c8331f9feea8c34cb9e0d6a179bb34918))
 * **[#999](https://github.com/cBournhonesque/lightyear/issues/999)**
    - Cb/0.20 ([`e7a259a`](https://github.com/cBournhonesque/lightyear/commit/e7a259ab58b6c0caa10d7f7924d443268b2a456d))
 * **Uncategorized**
    - Release lightyear_replication v0.21.0-rc.2, lightyear_sync v0.21.0-rc.2, lightyear_interpolation v0.21.0-rc.2, lightyear_prediction v0.21.0-rc.2, lightyear_frame_interpolation v0.21.0-rc.2, lightyear_avian v0.21.0-rc.2, lightyear_crossbeam v0.21.0-rc.2, lightyear_inputs v0.21.0-rc.2, lightyear_inputs_bei v0.21.0-rc.2, lightyear_inputs_leafwing v0.21.0-rc.2, lightyear_inputs_native v0.21.0-rc.2, lightyear_netcode v0.21.0-rc.2, lightyear_steam v0.21.0-rc.2, lightyear_webtransport v0.21.0-rc.2, lightyear_udp v0.21.0-rc.2, lightyear v0.21.0-rc.2 ([`b6dc58a`](https://github.com/cBournhonesque/lightyear/commit/b6dc58ac14426fb5ed211fc07af46be137a3cb34))
    - Release lightyear_messages v0.21.0-rc.2, lightyear_replication v0.21.0-rc.2, lightyear_sync v0.21.0-rc.2, lightyear_interpolation v0.21.0-rc.2, lightyear_prediction v0.21.0-rc.2, lightyear_frame_interpolation v0.21.0-rc.2, lightyear_avian v0.21.0-rc.2, lightyear_crossbeam v0.21.0-rc.2, lightyear_inputs v0.21.0-rc.2, lightyear_inputs_bei v0.21.0-rc.2, lightyear_inputs_leafwing v0.21.0-rc.2, lightyear_inputs_native v0.21.0-rc.2, lightyear_netcode v0.21.0-rc.2, lightyear_steam v0.21.0-rc.2, lightyear_webtransport v0.21.0-rc.2, lightyear_udp v0.21.0-rc.2, lightyear v0.21.0-rc.2 ([`156d504`](https://github.com/cBournhonesque/lightyear/commit/156d5044566e38244b1761401e799f33f84009bb))
    - Release lightyear_transport v0.21.0-rc.2, lightyear_messages v0.21.0-rc.2, lightyear_replication v0.21.0-rc.2, lightyear_sync v0.21.0-rc.2, lightyear_interpolation v0.21.0-rc.2, lightyear_prediction v0.21.0-rc.2, lightyear_frame_interpolation v0.21.0-rc.2, lightyear_avian v0.21.0-rc.2, lightyear_crossbeam v0.21.0-rc.2, lightyear_inputs v0.21.0-rc.2, lightyear_inputs_bei v0.21.0-rc.2, lightyear_inputs_leafwing v0.21.0-rc.2, lightyear_inputs_native v0.21.0-rc.2, lightyear_netcode v0.21.0-rc.2, lightyear_steam v0.21.0-rc.2, lightyear_webtransport v0.21.0-rc.2, lightyear_udp v0.21.0-rc.2, lightyear v0.21.0-rc.2 ([`a52b3b8`](https://github.com/cBournhonesque/lightyear/commit/a52b3b89dcbdf7dc99d55255c37bb1197f906abd))
    - Release lightyear_transport v0.21.0-rc.2, lightyear_messages v0.21.0-rc.2, lightyear_replication v0.21.0-rc.2, lightyear_sync v0.21.0-rc.2, lightyear_interpolation v0.21.0-rc.2, lightyear_prediction v0.21.0-rc.2, lightyear_frame_interpolation v0.21.0-rc.2, lightyear_avian v0.21.0-rc.2, lightyear_crossbeam v0.21.0-rc.2, lightyear_inputs v0.21.0-rc.2, lightyear_inputs_bei v0.21.0-rc.2, lightyear_inputs_leafwing v0.21.0-rc.2, lightyear_inputs_native v0.21.0-rc.2, lightyear_netcode v0.21.0-rc.2, lightyear_steam v0.21.0-rc.2, lightyear_webtransport v0.21.0-rc.2, lightyear_udp v0.21.0-rc.2, lightyear v0.21.0-rc.2 ([`af910bc`](https://github.com/cBournhonesque/lightyear/commit/af910bc2c162ec521b55003610a54023f6c034ce))
    - Release lightyear_transport v0.21.0-rc.2, lightyear_messages v0.21.0-rc.2, lightyear_replication v0.21.0-rc.2, lightyear_sync v0.21.0-rc.2, lightyear_interpolation v0.21.0-rc.2, lightyear_prediction v0.21.0-rc.2, lightyear_frame_interpolation v0.21.0-rc.2, lightyear_avian v0.21.0-rc.2, lightyear_crossbeam v0.21.0-rc.2, lightyear_inputs v0.21.0-rc.2, lightyear_inputs_bei v0.21.0-rc.2, lightyear_inputs_leafwing v0.21.0-rc.2, lightyear_inputs_native v0.21.0-rc.2, lightyear_netcode v0.21.0-rc.2, lightyear_steam v0.21.0-rc.2, lightyear_webtransport v0.21.0-rc.2, lightyear_udp v0.21.0-rc.2, lightyear v0.21.0-rc.2 ([`244077f`](https://github.com/cBournhonesque/lightyear/commit/244077f9e729f0c267e6b865c244ac915f6d244f))
    - Release lightyear_serde v0.21.0-rc.2, lightyear_utils v0.21.0-rc.2, lightyear_core v0.21.0-rc.2, lightyear_link v0.21.0-rc.2, lightyear_aeronet v0.21.0-rc.2, lightyear_connection v0.21.0-rc.2, lightyear_macros v0.21.0-rc.2, lightyear_transport v0.21.0-rc.2, lightyear_messages v0.21.0-rc.2, lightyear_replication v0.21.0-rc.2, lightyear_sync v0.21.0-rc.2, lightyear_interpolation v0.21.0-rc.2, lightyear_prediction v0.21.0-rc.2, lightyear_frame_interpolation v0.21.0-rc.2, lightyear_avian v0.21.0-rc.2, lightyear_crossbeam v0.21.0-rc.2, lightyear_inputs v0.21.0-rc.2, lightyear_inputs_bei v0.21.0-rc.2, lightyear_inputs_leafwing v0.21.0-rc.2, lightyear_inputs_native v0.21.0-rc.2, lightyear_netcode v0.21.0-rc.2, lightyear_steam v0.21.0-rc.2, lightyear_webtransport v0.21.0-rc.2, lightyear_udp v0.21.0-rc.2, lightyear v0.21.0-rc.2 ([`89f1549`](https://github.com/cBournhonesque/lightyear/commit/89f1549f6d9e79719561dadaa8ff1f8d6772f77d))
    - Add release command to ci ([`cedab05`](https://github.com/cBournhonesque/lightyear/commit/cedab052a0f47cf91b15267b8d83eb87524a8f4d))
    - Run cargo fmt ([`4ae9ac1`](https://github.com/cBournhonesque/lightyear/commit/4ae9ac16922d9c160bfb01733a28749a78bfcb3a))
    - Fix clippy ([`249b40f`](https://github.com/cBournhonesque/lightyear/commit/249b40f358977f6f85e269967d3912bfb4080f73))
    - Cargo fmt ([`f55c117`](https://github.com/cBournhonesque/lightyear/commit/f55c117c1627368978d26c788efbcb2ddda1da01))
    - Make fps example compile ([`31f9102`](https://github.com/cBournhonesque/lightyear/commit/31f9102e17b76b5088f8f5f3be8048ce853363b2))
</details>

## v0.21.0-rc.1 (2025-06-08)

<csr-id-f361b72d433086c61ed6b4776fd4ee308c3747e1/>

### Chore

 - <csr-id-f361b72d433086c61ed6b4776fd4ee308c3747e1/> add changelogs

