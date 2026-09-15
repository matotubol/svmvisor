# Agent working requirements

Immediate user priority, clarified 2026-09-13: first boot Windows under the
native DXE hypervisor on the existing machine. Malware-analysis features and
a general emulated platform are deferred until after that milestone. Existing
emulator fixtures are validation tools, not the delivery goal. Prioritize native
firmware/loader continuation, resident memory lifetime and the execution policy
needed for normal boot. Do not promote future containment, telemetry, card-hiding
or attestation features into prerequisites for this trusted first-boot work.
Preserve stopped-state correctness and report unsupported configurations honestly.

Read README.md, CONTRIBUTING.md, the applicable crate guide, and
docs/malware-analysis-direction.md before implementation or review.
The intended product is a hypervisor-based malware-analysis platform. Faithful
execution, containment, Windows protection compatibility, timing observability,
and honest analysis coverage are architecture requirements from the start.

For every relevant implementation batch, apply the exit-policy, timing and
compatibility requirements in docs/malware-analysis-direction.md. Record what
was implemented, what was actually measured, and what remains unsupported.
Do not equate successful Windows boot with sandbox readiness or undetectability.

When delegating work, include these requirements and assign concrete ownership.
Keep architecture cleanup part of implementation: reuse existing owners,
consolidate duplicated setup, and remove superseded paths within the batch.
Every added abstraction must have a real caller; avoid empty future layers.
Reviewers must check them as well as functional correctness. User-requested
parallel work should use fresh agents for independent implementation/review.
For complex architecture and implementation batches, the user's model
preference is GPT-6 Astra (`gpt-6-astra`) with High reasoning. Start these
agents with explicit ownership and enough context to work independently.
Missing documentation or uncertain architectural facts must be identified
explicitly and checked against current primary sources. Cloning repositories
for efficient source inspection is permitted; pin the revision used and keep
reference checkouts separate from implementation. Do not overwrite dirty work.

User instruction, 2026-09-16: all implementation work must run directly in
C:\Users\mato\Documents\svmvisor on main. Do not create or switch to worktrees.
Start with docs/handoff-2026-09-16.md. The older 7a58 worktree is retained
only as a read-only archive of local research and build evidence.
Physical results apply
only to the exact tested image; emulator results do not inherit physical proof.

The user's reference library is C:\Users\mato\Documents\svmvisor\docs.
Consult relevant manuals there before declaring documentation missing,
including the processor-specific PPR/BKDG PDFs. Worktree copies may be used
when their hashes match; record document revision and product applicability.
The reference location does not change the authoritative implementation path.

User clarification, 2026-09-14: critical manual reviews must use rendered PDF
page images, not extracted PDF text. Read complete relevant tables, merged
cells, footnotes and adjacent explanations visually; record document hash,
revision, applicability, PDF page and printed page. Existing extracted summaries
are not authoritative evidence and must be rechecked before relying on them.
Follow relevant manual cross-references to their referenced sections, tables,
figures, footnotes and page images, including references into other volumes.
Record the reference chain and any unresolved dependency; do not declare a
critical rule verified from its initial page alone when it delegates conditions
or exceptions elsewhere. Distinguish printed page numbers from PDF page indices.

PI 1.10 is now available in that library as UEFI_PI_Spec_1_10.pdf, SHA256
ed35ab171e8aa66514e2f04013faf7912098b960bdf614f973a8b9d4a5ff09ea.
Use its actual MP Services chapter for new work; the verified section/page map
is work/native-percpu/pi-1.10-review.md. Earlier unavailable-PI notes are
historical and do not mean this current PDF is missing.


