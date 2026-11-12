# Licensing and third-party materials

## Project license

The intended project license is Apache License 2.0. Before the first public release, add the canonical `LICENSE` text, copyright notices, and an appropriate `NOTICE` file. Every source file should use consistent project copyright/SPDX metadata as determined by maintainers. Do not claim the code is already licensed until those files are present.

Apache 2.0 applies to Audora's own work; it does not automatically change third-party dependency licenses. Release assets must include required license texts and notices, and must not impose additional restrictions inconsistent with applicable licenses.

## Dependency policy

- Prefer permissive dependencies compatible with Apache 2.0 obligations (for example MIT, BSD, ISC, Apache-2.0) where technical quality is comparable.
- LGPL dependencies may be used only after legal/license review of the exact linking, modification, relinking, notice, and source-offer obligations. The proposed GStreamer integration and bundled plugin set require specific review.
- GPL/AGPL dependencies are excluded from distributed application binaries unless maintainers explicitly revisit the product licensing decision and document the consequences.
- Every direct and transitive native dependency, bundled plugin, font, icon, and installer component must be inventoried. License scanner output is evidence for review, not an automatic legal conclusion.
- Pin or constrain dependency versions and retain required source/modification notices for distributed components.

## Media formats and codecs

Container and codec patent rules vary by jurisdiction and distribution model. Do not state that a codec is patent-free or universally distributable without qualified review. Publish the exact supported format matrix and whether a decoder is bundled or supplied by the operating system. Make any optional codec/plugin installation explicit and link to its license information.

## Release checklist

1. Include Apache-2.0 license and project notices.
2. Generate a release-specific dependency and plugin inventory/SBOM.
3. Validate bundled components' license texts and notices for all target packages.
4. Review codec/plugin redistribution and patent exposure with qualified counsel before broad distribution.
5. Preserve attribution and source offer/relocation obligations where applicable.
6. Re-run review whenever dependencies, packaging, plugin set, or linking mode changes.
