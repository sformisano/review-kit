"use strict";

const learningExamples = {
  findingParts: {
    identity: {
      type: "FindingId",
      value: "id: 1",
      heading: "A handle for the same problem",
      explanation: "FindingId holds a positive number. F1 is its display label. The ID stays the same after reassessment and is local to this subject and reviewer."
    },
    location: {
      type: "SourceLocation",
      value: "path: SourcePath\n  src/worker.rs\nline: LineNumber\n  42",
      heading: "Two values describe the anchor",
      explanation: "SourceLocation combines a repository-relative SourcePath and a one-based LineNumber. Their constructors check path shape and a positive line number. Source existence needs a check against the reviewed revision."
    },
    judgment: {
      type: "Severity · Disposition · FindingStatus",
      value: "severity: P2\ndisposition: Blocking\nstatus: Open\nvalidated: true",
      heading: "Different questions need different fields",
      explanation: "Severity records how serious the problem is. Disposition records whether it blocks the review. Status records whether the problem remains. The agent supplies these independently. For this open blocking finding, rkc sets validated only when a recorded independent check covers its exact version; the record is not proof that the check ran."
    },
    evidence: {
      type: "String",
      value: "evidence:\n  A failed send removes\n  the job from the queue.",
      heading: "State what supports the finding",
      explanation: "FindingDraft holds the agent’s title, body, and evidence. Finding::new rejects blank text in those fields. A nonblank explanation can still be wrong; the constructor does not investigate the code."
    },
    suggestion: {
      type: "Option<Suggestion>",
      value: "revision: a1\nlocation: src/worker.rs:42\noriginal: queue.ack(job.id)\nreplacement: queue.retry(job.id)",
      heading: "An optional correction tied to source",
      explanation: "Suggestion holds the anchor, revision, original lines, replacement lines, and correction evidence. The lines above are abbreviated. verify_original compares supplied source exactly; the application obtains that source through a port."
    }
  },
  lifecycle: {
    open: {
      revision: "a1", status: "Open", style: "open",
      evidence: "The error branch acknowledges a failed job.",
      foot: "No transition yet.", direction: "No change yet",
      explanation: "Choose “Apply the fix” to record a status change and its explanation.",
      equal: true, equalityReason: "Same recorded state."
    },
    fixed: {
      revision: "b2", status: "Fixed", style: "fixed",
      evidence: "The error branch retries the job; the failure test passes.",
      foot: "ID, source anchor, and any suggestion are retained.",
      direction: "Open → Fixed",
      explanation: "Finding::transition replaces status and evidence, then returns the change with its explanation. It performs no provider writes.",
      equal: false, equalityReason: "The status and evidence differ."
    },
    recurrence: {
      revision: "c3", status: "Open", style: "open",
      evidence: "A new error branch acknowledges failed work again.",
      foot: "F1 returns to Open. Its ID is still 1.",
      direction: "Fixed → Open",
      explanation: "This example first fixes F1, then reopens it. FindingTransition::is_recurrence is true when Fixed or Withdrawn becomes Open.",
      equal: false, equalityReason: "The status matches the first review; the evidence differs."
    }
  },
  selections: {
    "error-handling": {
      rows: [["core-review", "Applicable"], ["error-handling", "Applicable"], ["test-evidence", "Applicable"]]
    },
    none: { rows: [["core-review", "Applicable"]] },
    unknown: { rows: [["core-review", "Applicable"], ["extra-rule", "Applicable; rkc reads no definition"]] }
  },
  assessments: {
    original: {
      revision: "a1", decision: "ChangesRequired", status: "Open", style: "open",
      evaluation: "Violated", evaluationStyle: "open",
      evidence: "Violation evidence: failed sends remove unfinished jobs.",
      accepted: true,
      explanation: "Every selected policy has one evaluation. F1 has a selected policy and a unique ID. ChangesRequired is consistent with the violation and open blocking finding."
    },
    premature: {
      revision: "a1", decision: "Ready", status: "Open", style: "open",
      evaluation: "Violated", evaluationStyle: "open",
      evidence: "Violation evidence: failed sends remove unfinished jobs.",
      accepted: false,
      errors: [
        ["The selected policy is violated.", "ReadyWithViolation(error-handling)"],
        ["F1 is open and blocking.", "ReadyWithBlockingFinding(1)"]
      ]
    },
    fixed: {
      revision: "b2", decision: "Ready", status: "Fixed", style: "fixed",
      evaluation: "Satisfied", evaluationStyle: "satisfied",
      evidence: "The failure test verifies retry behavior in the example.",
      accepted: true,
      explanation: "All selected policies are satisfied. F1 is fixed. The agent supplies Ready; the constructor accepts this combination. Severity does not calculate the decision."
    },
    missing: {
      revision: "a1", decision: "ChangesRequired", status: "Open", style: "open",
      evaluation: "Violated", evaluationStyle: "open", missingEvaluation: true,
      evidence: "Violation evidence: failed sends remove unfinished jobs.",
      accepted: false,
      errors: [["test-evidence is selected but has no evaluation.", "MissingEvaluation(test-evidence)"]]
    },
    blocked: {
      revision: "b2", decision: "Blocked", status: "Uncertain", style: "uncertain",
      evaluation: "Blocked", evaluationStyle: "blocked",
      evidence: "Policy limitation: the failure test cannot run in this environment.",
      limitation: "Overall limitation: retry behavior remains unverified.",
      accepted: true,
      explanation: "The uncertain finding and blocked evaluation require a Blocked decision. Both the policy limitation and the overall limitation are present."
    },
    "no-limitation": {
      revision: "b2", decision: "Blocked", status: "Uncertain", style: "uncertain",
      evaluation: "Blocked", evaluationStyle: "blocked",
      evidence: "Policy limitation: the failure test cannot run in this environment.",
      limitation: "Overall limitation: missing",
      accepted: false,
      errors: [["A Blocked decision also needs a nonblank overall limitation.", "MissingBlockedLimitation"]]
    }
  },
  publications: {
    initial: [
      { label: "Check the assessment", actor: "Application + domain", title: "Start with a checked record", description: "The agent supplies the review. Before publishing, the application checks declared results, source, and rendering through its ports.", observation: "Assessment: ChangesRequired. F1: Open. No provider writes have occurred.", lesson: "A valid assessment is the input to publication. It does not mean a comment has been posted." },
      { label: "Discover current state", actor: "Application → GitLab adapter", title: "Read before planning writes", description: "Discover the request revision, existing owned review records, authenticated reviewer, and available authority.", observation: "The reviewed head is still current. The example has no prior owned review.", lesson: "Provider observations determine the plan. A marker alone does not establish comment ownership." },
      { label: "Verify pending summary", actor: "Application → GitLab adapter", title: "Make the round visible first", description: "Create the round’s Pending summary and read it back. Proceed to finding writes only after verifying that summary.", observation: "Round 1: Pending. Its summary is verified. F1 has no provider discussion yet.", lesson: "This ordering leaves a visible record if publication stops after a later write." },
      { label: "Verify F1", actor: "Application → GitLab adapter", title: "Post the finding and verify it", description: "Create the source discussion for F1, then verify the stored effect. Keep the returned discussion and root-note IDs separate from FindingId.", observation: "F1 is verified in a provider discussion. The summary remains Pending.", lesson: "A successful dispatch alone is not a completed publication." },
      { label: "Verify completed summary", actor: "Application → GitLab adapter", title: "Complete the publication", description: "Update the summary with verified finding links and completed round state, then verify that update.", observation: "Round 1: Completed. ReviewDecision: ChangesRequired. F1: Open.", lesson: "Completed means the communication finished. The code still needs a fix." }
    ],
    unchanged: [
      { label: "Retain the completed review", actor: "Before the rerun", title: "The same communication already exists", description: "The earlier publication completed. The new assessment describes structurally unchanged communication for the same review.", observation: "Round 1 is Completed. Its summary and F1 already exist.", lesson: "Unchanged means the relevant review content matches, not merely that the head string matches." },
      { label: "Discover and compare", actor: "Application → GitLab adapter", title: "Verify the existing communication", description: "Discover the owned provider records and compare the completed communication with the requested review.", observation: "The existing completed review matches. There is no unresolved write intent in this example.", lesson: "Discovery is a read. A rerun does not inherently require another comment." },
      { label: "Keep the existing round", actor: "Application", title: "Finish without writes", description: "Reuse the completed review. Do not create another summary, finding, reply, or round.", observation: "Zero writes. Round 1 remains the current completed round.", lesson: "The unchanged-review test checks both the absence of writes and the preserved round number." }
    ],
    fix: [
      { label: "Reassess F1", actor: "Agent + domain", title: "Bring a fresh assessment", description: "The agent verifies the fix at b2, keeps FindingId 1, and supplies the Open-to-Fixed explanation for the follow-up.", observation: "New assessment: Ready. F1: Fixed. The earlier assessment still records F1 as Open.", lesson: "The domain transition itself has not posted a reply or resolved the provider thread." },
      { label: "Verify pending follow-up", actor: "Application → GitLab adapter", title: "Plan the next round", description: "Discover current state and verify the summary update that records the new round as Pending.", observation: "Round 2: Pending. F1’s existing discussion is still open.", lesson: "The earlier round stays in the summary’s history." },
      { label: "Verify transition reply", actor: "Application → GitLab adapter", title: "Explain the fix in F1’s thread", description: "Post the authored transition reply to the existing finding discussion and verify it before attempting resolution.", observation: "The fix explanation is verified. The discussion is still open.", lesson: "Readers can see why the finding changed before the thread is resolved." },
      { label: "Verify resolution", actor: "Application → GitLab adapter", title: "Resolve the existing discussion", description: "Resolve F1’s discussion and verify the resulting provider state.", observation: "The same F1 discussion is resolved. No replacement finding was created.", lesson: "A recurrence uses an authored reply before reopening the existing discussion." },
      { label: "Verify completed follow-up", actor: "Application → GitLab adapter", title: "Record the completed round", description: "Finalize and verify the summary after the ordered follow-up effects complete.", observation: "Round 2: Completed. ReviewDecision: Ready. F1: Fixed.", lesson: "Finding identity links the problem across both assessments and publication rounds." }
    ],
    lost: [
      { label: "Verify pending summary", actor: "Application → GitLab adapter", title: "Start after the pending barrier", description: "Checking and discovery have passed. The round’s pending summary has been created and verified.", observation: "Round 1: Pending. F1 has not yet been sent.", lesson: "This case follows the finding write; earlier checks are omitted from the trace." },
      { label: "Send F1; lose response", actor: "GitLab adapter", title: "The POST outcome is unknown", description: "The finding POST is sent, but its response is lost. Retained PublicationProgress carries the exact unresolved intent.", observation: "F1 may have been created. The response cannot establish success or rejection.", lesson: "An unknown outcome does not authorize another POST.", unknown: true },
      { label: "Discover exact owned effect", actor: "Application → GitLab adapter", title: "Find the effect without resending", description: "Discovery finds an API-owned, marked finding that matches the retained creation intent exactly. The application verifies that effect.", observation: "F1 is verified. The finding POST was sent once.", lesson: "If the effect is absent, the default policy permits one eligible resend after complete reconciliation. The conservative option preserves uncertainty." },
      { label: "Verify completed summary", actor: "Application → GitLab adapter", title: "Continue after corroboration", description: "With F1 verified, finalize and verify the completed summary. Recovery can occur in the same call or a later call with retained progress.", observation: "Round 1: Completed. F1 was created once and verified through discovery.", lesson: "Recovery requires retained intent and matching provider evidence. It does not promise exactly-once delivery." }
    ],
    "unknown-update": [
      { label: "Verify pending and F1", actor: "Application → GitLab adapter", title: "The earlier effects are known", description: "The pending summary and F1 have been written and verified. The remaining effect is the final summary update.", observation: "Round 1: Pending. F1 is verified.", lesson: "This case starts at finalization; the earlier publication steps are omitted." },
      { label: "Send summary PUT; lose response", actor: "GitLab adapter", title: "Retain the unresolved update", description: "The final summary PUT is sent, but the response is lost. The caller retains PublicationProgress with this unresolved update.", observation: "The publication result is Uncertain. The final update’s outcome is unresolved.", lesson: "A PUT’s matching visible state does not establish which write caused it.", unknown: true },
      { label: "Discover matching state", actor: "Application → GitLab adapter", title: "Accept the verified result", description: "Discovery sees the exact owned summary and Completed state recorded in retained intent. Recovery settles that observed result.", observation: "Visible summary: Completed. Retained update: settled by observation.", lesson: "The requested state is verified. This does not prove which request caused it." },
      { label: "Reuse completed review", actor: "Application", title: "A rerun performs no writes", description: "The caller supplies retained progress on a rerun. The application verifies completed communication without sending the update again.", observation: "Zero writes on the rerun. The existing completed round is unchanged.", lesson: "Retained intent and exact provider observations allow recovery without claiming exactly-once delivery." }
    ]
  }
};
