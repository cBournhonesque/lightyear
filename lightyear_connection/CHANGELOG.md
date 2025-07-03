# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## v0.21.0-rc.3 (2025-07-03)

### Chore

 - <csr-id-5dc2e81f8c2b1171df33703d73e38a49e7b4695d/> release rc3
 - <csr-id-81341e91707b31a5cba6967d23e230945180a4e8/> release 0.21 rc 2
 - <csr-id-fe0bb4a24112a308eaf9c829fe5cfae0180ef946/> fix tests, cargo doc, cargo clippy
 - <csr-id-249b40f358977f6f85e269967d3912bfb4080f73/> fix clippy
 - <csr-id-f55c117c1627368978d26c788efbcb2ddda1da01/> cargo fmt
 - <csr-id-bc7cf371f822ff7a2667c329b6f77e5a694a93d4/> enable host-server for all examples
 - <csr-id-411733089f59eb90d405f7ad327b5440b55ef060/> enable host-client mode on simple box

### New Features

 - <csr-id-0bd3fbe9db6d8dfd350a0e014e7beec9392df1de/> enable steam on simple_box example and fix wasm
 - <csr-id-117b0841a25dba5c6ffaadad88a8c4dba09d3cbb/> support BEI inputs

### Bug Fixes

 - <csr-id-a0667fa3099df9f11f4304a97104f148bc0be22d/> fix host-server disconnect

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 16 commits contributed to the release over the course of 45 calendar days.
 - 10 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 12 unique issues were worked on: [#1017](https://github.com/cBournhonesque/lightyear/issues/1017), [#1018](https://github.com/cBournhonesque/lightyear/issues/1018), [#1021](https://github.com/cBournhonesque/lightyear/issues/1021), [#1023](https://github.com/cBournhonesque/lightyear/issues/1023), [#1024](https://github.com/cBournhonesque/lightyear/issues/1024), [#1029](https://github.com/cBournhonesque/lightyear/issues/1029), [#1031](https://github.com/cBournhonesque/lightyear/issues/1031), [#1039](https://github.com/cBournhonesque/lightyear/issues/1039), [#1043](https://github.com/cBournhonesque/lightyear/issues/1043), [#1055](https://github.com/cBournhonesque/lightyear/issues/1055), [#1061](https://github.com/cBournhonesque/lightyear/issues/1061), [#989](https://github.com/cBournhonesque/lightyear/issues/989)

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

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
 * **[#1031](https://github.com/cBournhonesque/lightyear/issues/1031)**
    - Fix host-server disconnect ([`a0667fa`](https://github.com/cBournhonesque/lightyear/commit/a0667fa3099df9f11f4304a97104f148bc0be22d))
 * **[#1039](https://github.com/cBournhonesque/lightyear/issues/1039)**
    - Support BEI inputs ([`117b084`](https://github.com/cBournhonesque/lightyear/commit/117b0841a25dba5c6ffaadad88a8c4dba09d3cbb))
 * **[#1043](https://github.com/cBournhonesque/lightyear/issues/1043)**
    - Make workspace crates depend on individual bevy crates ([`5dc3dc3`](https://github.com/cBournhonesque/lightyear/commit/5dc3dc3e17a8b821c35162b904b73eea0e1c69be))
 * **[#1055](https://github.com/cBournhonesque/lightyear/issues/1055)**
    - Release 0.21 rc 2 ([`81341e9`](https://github.com/cBournhonesque/lightyear/commit/81341e91707b31a5cba6967d23e230945180a4e8))
 * **[#1061](https://github.com/cBournhonesque/lightyear/issues/1061)**
    - Enable steam on simple_box example and fix wasm ([`0bd3fbe`](https://github.com/cBournhonesque/lightyear/commit/0bd3fbe9db6d8dfd350a0e014e7beec9392df1de))
 * **[#989](https://github.com/cBournhonesque/lightyear/issues/989)**
    - Bevy main refactor ([`b236123`](https://github.com/cBournhonesque/lightyear/commit/b236123c8331f9feea8c34cb9e0d6a179bb34918))
 * **Uncategorized**
    - Release rc3 ([`5dc2e81`](https://github.com/cBournhonesque/lightyear/commit/5dc2e81f8c2b1171df33703d73e38a49e7b4695d))
    - Fix tests, cargo doc, cargo clippy ([`fe0bb4a`](https://github.com/cBournhonesque/lightyear/commit/fe0bb4a24112a308eaf9c829fe5cfae0180ef946))
    - Fix clippy ([`249b40f`](https://github.com/cBournhonesque/lightyear/commit/249b40f358977f6f85e269967d3912bfb4080f73))
    - Cargo fmt ([`f55c117`](https://github.com/cBournhonesque/lightyear/commit/f55c117c1627368978d26c788efbcb2ddda1da01))
</details>

## v0.21.0-rc.2 (2025-06-30)

<csr-id-cedab052a0f47cf91b15267b8d83eb87524a8f4d/>
<csr-id-fe0bb4a24112a308eaf9c829fe5cfae0180ef946/>
<csr-id-249b40f358977f6f85e269967d3912bfb4080f73/>
<csr-id-f55c117c1627368978d26c788efbcb2ddda1da01/>
<csr-id-bc7cf371f822ff7a2667c329b6f77e5a694a93d4/>
<csr-id-411733089f59eb90d405f7ad327b5440b55ef060/>

### Chore

 - <csr-id-cedab052a0f47cf91b15267b8d83eb87524a8f4d/> add release command to ci
 - <csr-id-fe0bb4a24112a308eaf9c829fe5cfae0180ef946/> fix tests, cargo doc, cargo clippy
 - <csr-id-249b40f358977f6f85e269967d3912bfb4080f73/> fix clippy
 - <csr-id-f55c117c1627368978d26c788efbcb2ddda1da01/> cargo fmt
 - <csr-id-bc7cf371f822ff7a2667c329b6f77e5a694a93d4/> enable host-server for all examples
 - <csr-id-411733089f59eb90d405f7ad327b5440b55ef060/> enable host-client mode on simple box

### New Features

 - <csr-id-117b0841a25dba5c6ffaadad88a8c4dba09d3cbb/> support BEI inputs

### Bug Fixes

 - <csr-id-a0667fa3099df9f11f4304a97104f148bc0be22d/> fix host-server disconnect

## v0.21.0-rc.1 (2025-06-08)

<csr-id-f361b72d433086c61ed6b4776fd4ee308c3747e1/>

### Chore

 - <csr-id-f361b72d433086c61ed6b4776fd4ee308c3747e1/> add changelogs

