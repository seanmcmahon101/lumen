# Code signing policy

Lumen is applying to the [SignPath Foundation](https://signpath.org/apply) for
free Windows code signing. Approval and account configuration are still pending;
the existing release downloads are unsigned. The policy below describes the
release process being prepared for that service. It does not claim that current
downloads have already been signed.

Free code signing provided by [SignPath.io](https://signpath.io), certificate by
[SignPath Foundation](https://signpath.org), once the application is approved.

## Responsibilities

[Sean McMahon](https://github.com/seanmcmahon101) is the maintainer, author,
reviewer and signing approver. Contributions from other people are reviewed
before release. SignPath and GitHub accounts used to release Lumen must use
multi-factor authentication.

Only official Lumen builds from
[this repository](https://github.com/seanmcmahon101/lumen) may be signed.
The Windows release workflow builds the application from a version tag on a
GitHub-hosted runner. SignPath verifies the origin of the uploaded build artifacts.
Each signing request requires the maintainer's approval in SignPath.

The app, Inno Setup uninstaller and installer must have matching Lumen product
metadata, valid embedded Authenticode signatures, and trusted timestamps.
The workflow verifies the installed app and uninstaller as well as the download.
Once the SignPath credentials are configured, signing failures or invalid
signatures stop publication. Until then, the workflow publishes unsigned
installers and says so in the release notes. The fixed-name and versioned downloads
contain the same signed installer. SHA-256 checksums accompany them.

See [PRIVACY.md](PRIVACY.md) for what remains local and which actions contact
GitHub or Microsoft 365.

## Activate SignPath

1. Apply at [signpath.org/apply](https://signpath.org/apply) with repository and
   homepage `https://github.com/seanmcmahon101/lumen`, download page
   `https://github.com/seanmcmahon101/lumen/releases`, and privacy policy
   `https://github.com/seanmcmahon101/lumen/blob/main/PRIVACY.md`.
   The project uses the MIT license and GitHub Actions on GitHub-hosted runners.
2. Once approved, configure SignPath project `lumen` with signing policy
   `release-signing`, its production certificate, Sean as approver, and GitHub.com
   as a trusted build system linked to this repository. Follow SignPath's
   onboarding instructions for any GitHub app installation or repository access.
3. Add artifact configurations with slugs `payload` and `installer`, using
   [signing/payload.xml](signing/payload.xml) and
   [signing/installer.xml](signing/installer.xml). Both accept a required `version`
   parameter such as `0.5.1.0`. Enable SHA-256 Authenticode signing and trusted
   timestamping. Retain manual approval for each signing request.
4. Create a SignPath CI submitter token scoped to this project and policy. Save
   it as GitHub repository secret `SIGNPATH_API_TOKEN`; save the organization ID
   as `SIGNPATH_ORGANIZATION_ID`. Enter credentials directly in the services,
   never in source code, release notes or chat.
5. Run **Windows release** on `main` with tag `v0.5.1` and `validate_only` left
   false. Approve the payload signing request, then the installer request. Each
   request has a 30-minute approval window while its job runs.
6. After checking the live downloads, update this page's pending status and the
   installation note in the README to describe the now-active service.

## Existing downloads and future releases

New version tags (`vMAJOR.MINOR.PATCH`) use this signing process automatically;
the maintainer still approves the two requests in SignPath.

For existing versions, dispatch **Windows release** from `main`, supplying an
existing tag. It uses the current packaging tools with that tag's original
application source and locked dependencies. It rebuilds and replaces the EXE
assets, adds versioned names where needed, and regenerates checksums while
preserving the release's description and latest-release designation. It never
moves a Git tag. Signing changes file hashes; check any external package-manager
manifests before replacing an asset, and update their expected SHA-256 values.
Already-downloaded copies cannot be signed remotely; users need a fresh download.

The published tags present when this process was added are `v0.5.1`, `v0.5.0`,
`v0.4.4`, `v0.4.3`, `v0.4.2`, `v0.4.1`, `v0.4.0`, `v0.3.0` and `v0.2.0`.

For a packaging check before SignPath is available, dispatch the workflow with
`validate_only=true`. It uses a temporary certificate trusted only on that
disposable GitHub runner to test signatures, timestamps, installation, upgrade
and uninstallation. It never creates or edits a release. Its test artifacts are
explicitly labelled **not for distribution** and expire after three days. This
check does not establish public Windows trust or test SignPath account approval.

## Windows warnings

A trusted signature identifies the publisher and protects file integrity.
SmartScreen reputation is a separate Microsoft-controlled signal: newly signed
apps can still show warnings. Self-signed certificates do not solve public
download warnings. See
[Microsoft's SmartScreen guidance](https://learn.microsoft.com/windows/apps/package-and-deploy/smartscreen-reputation).
