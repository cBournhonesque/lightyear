# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## v0.21.0-rc.2 (2025-07-01)

<csr-id-cedab052a0f47cf91b15267b8d83eb87524a8f4d/>
<csr-id-72ecbb9604bbb7add8e911cf9d72f21fd00eed6c/>
<csr-id-f8d62c7389c6095694fcd8ceeb593b6d0ebc5b47/>
<csr-id-f9bc3e3d8322d252d80363f716d5e78782520cff/>
<csr-id-b9c22da58aac0aed5d99feb2d3e773582fcf27e4/>
<csr-id-4ae9ac16922d9c160bfb01733a28749a78bfcb3a/>
<csr-id-249b40f358977f6f85e269967d3912bfb4080f73/>
<csr-id-f55c117c1627368978d26c788efbcb2ddda1da01/>
<csr-id-bc7cf371f822ff7a2667c329b6f77e5a694a93d4/>
<csr-id-411733089f59eb90d405f7ad327b5440b55ef060/>

### Chore

 - <csr-id-cedab052a0f47cf91b15267b8d83eb87524a8f4d/> add release command to ci
 - <csr-id-72ecbb9604bbb7add8e911cf9d72f21fd00eed6c/> add tests for delta-compression
 - <csr-id-f8d62c7389c6095694fcd8ceeb593b6d0ebc5b47/> add more tests on replication internals, to prepare for fixing SinceLastSend
 - <csr-id-f9bc3e3d8322d252d80363f716d5e78782520cff/> fix ci
 - <csr-id-b9c22da58aac0aed5d99feb2d3e773582fcf27e4/> fix ci
 - <csr-id-4ae9ac16922d9c160bfb01733a28749a78bfcb3a/> run cargo fmt
 - <csr-id-249b40f358977f6f85e269967d3912bfb4080f73/> fix clippy
 - <csr-id-f55c117c1627368978d26c788efbcb2ddda1da01/> cargo fmt
 - <csr-id-bc7cf371f822ff7a2667c329b6f77e5a694a93d4/> enable host-server for all examples
 - <csr-id-411733089f59eb90d405f7ad327b5440b55ef060/> enable host-client mode on simple box

### New Features

 - <csr-id-117b0841a25dba5c6ffaadad88a8c4dba09d3cbb/> support BEI inputs

### Bug Fixes

 - <csr-id-e85935036975bb7bda4f2d77fb00df66084cc513/> fix bug on fps example with missing PlayerMarker component
 - <csr-id-1108da74e019d8efc37728b58ab07ac9472aaefa/> fix bug on fps example with missing PlayerMarker component

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 29 commits contributed to the release over the course of 43 calendar days.
 - 13 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 16 unique issues were worked on: [#1015](https://github.com/cBournhonesque/lightyear/issues/1015), [#1017](https://github.com/cBournhonesque/lightyear/issues/1017), [#1018](https://github.com/cBournhonesque/lightyear/issues/1018), [#1021](https://github.com/cBournhonesque/lightyear/issues/1021), [#1023](https://github.com/cBournhonesque/lightyear/issues/1023), [#1024](https://github.com/cBournhonesque/lightyear/issues/1024), [#1029](https://github.com/cBournhonesque/lightyear/issues/1029), [#1033](https://github.com/cBournhonesque/lightyear/issues/1033), [#1038](https://github.com/cBournhonesque/lightyear/issues/1038), [#1039](https://github.com/cBournhonesque/lightyear/issues/1039), [#1043](https://github.com/cBournhonesque/lightyear/issues/1043), [#1047](https://github.com/cBournhonesque/lightyear/issues/1047), [#1049](https://github.com/cBournhonesque/lightyear/issues/1049), [#1051](https://github.com/cBournhonesque/lightyear/issues/1051), [#1054](https://github.com/cBournhonesque/lightyear/issues/1054), [#989](https://github.com/cBournhonesque/lightyear/issues/989)

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **[#1015](https://github.com/cBournhonesque/lightyear/issues/1015)**
    - Allow replicating immutable components ([`fb48928`](https://github.com/cBournhonesque/lightyear/commit/fb489288e86fc3438d24f217fe4e82b33909e086))
 * **[#1017](https://github.com/cBournhonesque/lightyear/issues/1017)**
    - Release 0.21 rc1 ([`dc0e61e`](https://github.com/cBournhonesque/lightyear/commit/dc0e61e06fe68309ed8cbfdcdfead633ad567537))
 * **[#1018](https://github.com/cBournhonesque/lightyear/issues/1018)**
    - Separate Connected from LocalId/RemoteId ([`89ce3e7`](https://github.com/cBournhonesque/lightyear/commit/89ce3e705fb262fe819ac1d254468caf3fc5fce5))
 * **[#1021](https://github.com/cBournhonesque/lightyear/issues/1021)**
    - Fix lobby example (without HostServer) and add protocolhash ([`0beb664`](https://github.com/cBournhonesque/lightyear/commit/0beb664f0161f73e4a53c06530ae139078ed8763))
 * **[#1023](https://github.com/cBournhonesque/lightyear/issues/1023)**
    - Add HostServer ([`5b6af7e`](https://github.com/cBournhonesque/lightyear/commit/5b6af7edd3b41c05333d14dde258ea5e89c07c2d))
 * **[#1024](https://github.com/cBournhonesque/lightyear/issues/1024)**
    - Enable host-client mode on simple box ([`4117330`](https://github.com/cBournhonesque/lightyear/commit/411733089f59eb90d405f7ad327b5440b55ef060))
 * **[#1029](https://github.com/cBournhonesque/lightyear/issues/1029)**
    - Enable host-server for all examples ([`bc7cf37`](https://github.com/cBournhonesque/lightyear/commit/bc7cf371f822ff7a2667c329b6f77e5a694a93d4))
 * **[#1033](https://github.com/cBournhonesque/lightyear/issues/1033)**
    - Adds #[reflect(MapEntities)] to RelationshipSync. ([`3b631a2`](https://github.com/cBournhonesque/lightyear/commit/3b631a226cbbf00cffff6c34c32deab3320d727f))
 * **[#1038](https://github.com/cBournhonesque/lightyear/issues/1038)**
    - Adds #[reflect(Component)] to Replicate. ([`26a8423`](https://github.com/cBournhonesque/lightyear/commit/26a8423b4c9b2c2e573a424aee9d0c60ff61f05a))
 * **[#1039](https://github.com/cBournhonesque/lightyear/issues/1039)**
    - Support BEI inputs ([`117b084`](https://github.com/cBournhonesque/lightyear/commit/117b0841a25dba5c6ffaadad88a8c4dba09d3cbb))
 * **[#1043](https://github.com/cBournhonesque/lightyear/issues/1043)**
    - Make workspace crates depend on individual bevy crates ([`5dc3dc3`](https://github.com/cBournhonesque/lightyear/commit/5dc3dc3e17a8b821c35162b904b73eea0e1c69be))
 * **[#1047](https://github.com/cBournhonesque/lightyear/issues/1047)**
    - Fix bug on fps example with missing PlayerMarker component ([`e859350`](https://github.com/cBournhonesque/lightyear/commit/e85935036975bb7bda4f2d77fb00df66084cc513))
    - Fix bug on fps example with missing PlayerMarker component ([`1108da7`](https://github.com/cBournhonesque/lightyear/commit/1108da74e019d8efc37728b58ab07ac9472aaefa))
 * **[#1049](https://github.com/cBournhonesque/lightyear/issues/1049)**
    - Alternative replication system + fix delta-compression ([`4d5e690`](https://github.com/cBournhonesque/lightyear/commit/4d5e69072485faa3975543792a8e11be7608a0ea))
 * **[#1051](https://github.com/cBournhonesque/lightyear/issues/1051)**
    - Add tests for delta-compression ([`72ecbb9`](https://github.com/cBournhonesque/lightyear/commit/72ecbb9604bbb7add8e911cf9d72f21fd00eed6c))
 * **[#1054](https://github.com/cBournhonesque/lightyear/issues/1054)**
    - Chore(docs) ([`59b9f7e`](https://github.com/cBournhonesque/lightyear/commit/59b9f7eb37b036488d3ceab780074274074a9bd6))
 * **[#989](https://github.com/cBournhonesque/lightyear/issues/989)**
    - Bevy main refactor ([`b236123`](https://github.com/cBournhonesque/lightyear/commit/b236123c8331f9feea8c34cb9e0d6a179bb34918))
 * **Uncategorized**
    - Release lightyear_messages v0.21.0-rc.2, lightyear_replication v0.21.0-rc.2, lightyear_sync v0.21.0-rc.2, lightyear_interpolation v0.21.0-rc.2, lightyear_prediction v0.21.0-rc.2, lightyear_frame_interpolation v0.21.0-rc.2, lightyear_avian v0.21.0-rc.2, lightyear_crossbeam v0.21.0-rc.2, lightyear_inputs v0.21.0-rc.2, lightyear_inputs_bei v0.21.0-rc.2, lightyear_inputs_leafwing v0.21.0-rc.2, lightyear_inputs_native v0.21.0-rc.2, lightyear_netcode v0.21.0-rc.2, lightyear_steam v0.21.0-rc.2, lightyear_webtransport v0.21.0-rc.2, lightyear_udp v0.21.0-rc.2, lightyear v0.21.0-rc.2 ([`156d504`](https://github.com/cBournhonesque/lightyear/commit/156d5044566e38244b1761401e799f33f84009bb))
    - Release lightyear_transport v0.21.0-rc.2, lightyear_messages v0.21.0-rc.2, lightyear_replication v0.21.0-rc.2, lightyear_sync v0.21.0-rc.2, lightyear_interpolation v0.21.0-rc.2, lightyear_prediction v0.21.0-rc.2, lightyear_frame_interpolation v0.21.0-rc.2, lightyear_avian v0.21.0-rc.2, lightyear_crossbeam v0.21.0-rc.2, lightyear_inputs v0.21.0-rc.2, lightyear_inputs_bei v0.21.0-rc.2, lightyear_inputs_leafwing v0.21.0-rc.2, lightyear_inputs_native v0.21.0-rc.2, lightyear_netcode v0.21.0-rc.2, lightyear_steam v0.21.0-rc.2, lightyear_webtransport v0.21.0-rc.2, lightyear_udp v0.21.0-rc.2, lightyear v0.21.0-rc.2 ([`a52b3b8`](https://github.com/cBournhonesque/lightyear/commit/a52b3b89dcbdf7dc99d55255c37bb1197f906abd))
    - Release lightyear_transport v0.21.0-rc.2, lightyear_messages v0.21.0-rc.2, lightyear_replication v0.21.0-rc.2, lightyear_sync v0.21.0-rc.2, lightyear_interpolation v0.21.0-rc.2, lightyear_prediction v0.21.0-rc.2, lightyear_frame_interpolation v0.21.0-rc.2, lightyear_avian v0.21.0-rc.2, lightyear_crossbeam v0.21.0-rc.2, lightyear_inputs v0.21.0-rc.2, lightyear_inputs_bei v0.21.0-rc.2, lightyear_inputs_leafwing v0.21.0-rc.2, lightyear_inputs_native v0.21.0-rc.2, lightyear_netcode v0.21.0-rc.2, lightyear_steam v0.21.0-rc.2, lightyear_webtransport v0.21.0-rc.2, lightyear_udp v0.21.0-rc.2, lightyear v0.21.0-rc.2 ([`af910bc`](https://github.com/cBournhonesque/lightyear/commit/af910bc2c162ec521b55003610a54023f6c034ce))
    - Release lightyear_transport v0.21.0-rc.2, lightyear_messages v0.21.0-rc.2, lightyear_replication v0.21.0-rc.2, lightyear_sync v0.21.0-rc.2, lightyear_interpolation v0.21.0-rc.2, lightyear_prediction v0.21.0-rc.2, lightyear_frame_interpolation v0.21.0-rc.2, lightyear_avian v0.21.0-rc.2, lightyear_crossbeam v0.21.0-rc.2, lightyear_inputs v0.21.0-rc.2, lightyear_inputs_bei v0.21.0-rc.2, lightyear_inputs_leafwing v0.21.0-rc.2, lightyear_inputs_native v0.21.0-rc.2, lightyear_netcode v0.21.0-rc.2, lightyear_steam v0.21.0-rc.2, lightyear_webtransport v0.21.0-rc.2, lightyear_udp v0.21.0-rc.2, lightyear v0.21.0-rc.2 ([`244077f`](https://github.com/cBournhonesque/lightyear/commit/244077f9e729f0c267e6b865c244ac915f6d244f))
    - Release lightyear_serde v0.21.0-rc.2, lightyear_utils v0.21.0-rc.2, lightyear_core v0.21.0-rc.2, lightyear_link v0.21.0-rc.2, lightyear_aeronet v0.21.0-rc.2, lightyear_connection v0.21.0-rc.2, lightyear_macros v0.21.0-rc.2, lightyear_transport v0.21.0-rc.2, lightyear_messages v0.21.0-rc.2, lightyear_replication v0.21.0-rc.2, lightyear_sync v0.21.0-rc.2, lightyear_interpolation v0.21.0-rc.2, lightyear_prediction v0.21.0-rc.2, lightyear_frame_interpolation v0.21.0-rc.2, lightyear_avian v0.21.0-rc.2, lightyear_crossbeam v0.21.0-rc.2, lightyear_inputs v0.21.0-rc.2, lightyear_inputs_bei v0.21.0-rc.2, lightyear_inputs_leafwing v0.21.0-rc.2, lightyear_inputs_native v0.21.0-rc.2, lightyear_netcode v0.21.0-rc.2, lightyear_steam v0.21.0-rc.2, lightyear_webtransport v0.21.0-rc.2, lightyear_udp v0.21.0-rc.2, lightyear v0.21.0-rc.2 ([`89f1549`](https://github.com/cBournhonesque/lightyear/commit/89f1549f6d9e79719561dadaa8ff1f8d6772f77d))
    - Add release command to ci ([`cedab05`](https://github.com/cBournhonesque/lightyear/commit/cedab052a0f47cf91b15267b8d83eb87524a8f4d))
    - Add more tests on replication internals, to prepare for fixing SinceLastSend ([`f8d62c7`](https://github.com/cBournhonesque/lightyear/commit/f8d62c7389c6095694fcd8ceeb593b6d0ebc5b47))
    - Fix ci ([`f9bc3e3`](https://github.com/cBournhonesque/lightyear/commit/f9bc3e3d8322d252d80363f716d5e78782520cff))
    - Fix ci ([`b9c22da`](https://github.com/cBournhonesque/lightyear/commit/b9c22da58aac0aed5d99feb2d3e773582fcf27e4))
    - Run cargo fmt ([`4ae9ac1`](https://github.com/cBournhonesque/lightyear/commit/4ae9ac16922d9c160bfb01733a28749a78bfcb3a))
    - Fix clippy ([`249b40f`](https://github.com/cBournhonesque/lightyear/commit/249b40f358977f6f85e269967d3912bfb4080f73))
    - Cargo fmt ([`f55c117`](https://github.com/cBournhonesque/lightyear/commit/f55c117c1627368978d26c788efbcb2ddda1da01))
</details>

## v0.21.0-rc.1 (2025-06-08)

<csr-id-f361b72d433086c61ed6b4776fd4ee308c3747e1/>

### Chore

 - <csr-id-f361b72d433086c61ed6b4776fd4ee308c3747e1/> add changelogs

