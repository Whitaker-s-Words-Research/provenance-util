-------------------------- MODULE ProvIntegration --------------------------
(***************************************************************************)
(* One integration: gate in prepare-commit-msg -> editor -> commit ->      *)
(* post-commit cleanup of session records, while the sessions keep living  *)
(* (resume, re-archive) in other worktrees.                                *)
(*                                                                         *)
(* SPEC.md sections: "Sessions of an integration", "Integration trailers", *)
(* "When the gate refuses", "Git hooks" (post-commit).                     *)
(* Rules: R-INT-1 (cleanup reads the commit that was made),                *)
(*        R-INT-2 (delete a record only if the commit's trailers name it,  *)
(*                 and it is Archived with the hash of its group; no other *)
(*                 record: `prov doctor` lists it, a human drops it).      *)
(* Accepted limit: the human can change the message in the editor, or a    *)
(* session can be resumed before the commit. The commit is then not        *)
(* complete (CommitIsComplete fails); prov check reports it, and R-INT-2   *)
(* keeps the records.                                                      *)
(*                                                                         *)
(* Session lifecycle is abstracted to open <-> archived; ProvSessions      *)
(* checks that Archived really means archived. ver[s] stands for the       *)
(* archive hash: it changes each time the session is archived again.       *)
(***************************************************************************)
(***************************************************************************)
(* Premises: what this model takes as facts about other tools. Each names *)
(* a row of "Appendix: test evidence" in SPEC.md, or says "untested".      *)
(*                                                                         *)
(* P1  prepare-commit-msg runs for a merge, also with --no-verify, and     *)
(*     MERGE_HEAD exists then. Tested: git 2.53, "prepare-commit-msg       *)
(*     during git merge --no-verify".                                      *)
(* P2  git opens the editor after prepare-commit-msg, and the commit has   *)
(*     the message as the editor left it. Tested by the test               *)
(*     an_edited_merge_message_keeps_the_records.                          *)
(* P3  post-commit sees the commit that was made, with its trailers.       *)
(*     Tested by the same test.                                            *)
(* P4  A session can be resumed at any time, also while the editor is      *)
(*     open. Tested for resume in general (Claude Code 2.1.322); not with  *)
(*     an editor open.                                                     *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
    Named,          \* sessions the incoming commits name (trailers or notes)
    Others,         \* other sessions with records on the same branch
    MaxVer,
    VerifiedCleanup, \* TRUE = R-INT-1/2; FALSE = SPEC.md 0.1 cleanup
    HumanEdits       \* the human may delete trailer groups in the editor

Sessions == Named \cup Others

VARIABLES
    st,        \* "open" | "archived"
    ver,       \* archive hash (abstract)
    rec,       \* a live-state record exists
    phase,     \* "idle" | "editor" | "committed" | "done"
    trailers,  \* <<session, hash>> groups in the message (editable until commit)
    unsafe,    \* history: records deleted although they were still needed
    badCommit  \* history: the commit was made without a current trailer
               \* group for each named session

vars == <<st, ver, rec, phase, trailers, unsafe, badCommit>>

----------------------------------------------------------------------------
Init ==
    /\ st       = [s \in Sessions |-> "archived"]
    /\ ver      = [s \in Sessions |-> 1]
    /\ rec      = [s \in Sessions |-> TRUE]
    /\ phase    = "idle"
    /\ trailers = {}
    /\ unsafe   = {}
    /\ badCommit = FALSE

(* --- the sessions keep living ------------------------------------------ *)
(* Resume. A session with no record gets a new one (SPEC.md "Resuming").   *)
Resume(s) ==
    /\ st[s] = "archived"
    /\ st'  = [st EXCEPT ![s] = "open"]
    /\ rec' = [rec EXCEPT ![s] = TRUE]
    /\ UNCHANGED <<ver, phase, trailers, unsafe, badCommit>>

(* Ends again and is archived: new transcript, new hash.                   *)
Rearchive(s) ==
    /\ st[s] = "open" /\ ver[s] < MaxVer
    /\ st'  = [st EXCEPT ![s] = "archived"]
    /\ ver' = [ver EXCEPT ![s] = @ + 1]
    /\ UNCHANGED <<rec, phase, trailers, unsafe, badCommit>>

(* --- the human integrates ---------------------------------------------- *)
(* prepare-commit-msg: the gate passes only if every named session has a   *)
(* record and is Archived; it writes one trailer group per session.        *)
Gate ==
    /\ phase = "idle"
    /\ \A s \in Named : rec[s] /\ st[s] = "archived"
    /\ trailers' = {<<s, ver[s]>> : s \in Named}
    /\ phase'    = "editor"
    /\ UNCHANGED <<st, ver, rec, unsafe, badCommit>>

(* In the editor the human can delete trailer groups.                      *)
EditMessage ==
    /\ HumanEdits /\ phase = "editor"
    /\ \E t \in SUBSET trailers : t # trailers /\ trailers' = t
    /\ UNCHANGED <<st, ver, rec, phase, unsafe, badCommit>>

Abort ==
    /\ phase = "editor"
    /\ phase' = "idle" /\ trailers' = {}
    /\ UNCHANGED <<st, ver, rec, unsafe, badCommit>>

(* The message is complete: each named session is Archived, and the        *)
(* message has its trailer group with the current hash.                    *)
Complete == \A s \in Named : rec[s] /\ st[s] = "archived" /\ <<s, ver[s]>> \in trailers

(* The commit, with the message as the editor left it. Nothing checks it   *)
(* after the editor (accepted limit).                                      *)
Commit ==
    /\ phase = "editor"
    /\ phase' = "committed"
    /\ badCommit' = (badCommit \/ ~Complete)
    /\ UNCHANGED <<st, ver, rec, trailers, unsafe>>

(* post-commit cleanup.                                                    *)
(* SPEC.md 0.1: delete the records of the integration's sessions and of    *)
(* every other session on the branch.                                      *)
(* R-INT-1/2: read the trailers of the commit actually made. Delete a      *)
(* record only if a trailer group names it with its current hash, and it   *)
(* is Archived. Delete no other record.                                    *)
Deletable(s) ==
    IF ~VerifiedCleanup THEN rec[s]
    ELSE rec[s] /\ s \in Named /\ st[s] = "archived" /\ <<s, ver[s]>> \in trailers

(* A deletion is unsafe if the record is still needed. A named session: it *)
(* is Open, or its current archive is not the one the commit records.      *)
(* Another session: always, until a human drops it (prov session drop).    *)
Needed(s) ==
    IF s \in Named
    THEN st[s] = "open" \/ <<s, ver[s]>> \notin trailers
    ELSE TRUE

Cleanup ==
    /\ phase = "committed"
    /\ LET del == {s \in Sessions : Deletable(s)}
       IN /\ rec'    = [s \in Sessions |-> rec[s] /\ s \notin del]
          /\ unsafe' = unsafe \cup {s \in del : Needed(s)}
    /\ phase' = "done"
    /\ UNCHANGED <<st, ver, trailers, badCommit>>

Next ==
    \/ Gate \/ EditMessage \/ Abort \/ Commit \/ Cleanup
    \/ \E s \in Sessions : Resume(s) \/ Rearchive(s)

Spec == Init /\ [][Next]_vars

----------------------------------------------------------------------------
TypeOK ==
    /\ st \in [Sessions -> {"open", "archived"}]
    /\ ver \in [Sessions -> 1..MaxVer]
    /\ rec \in [Sessions -> BOOLEAN]
    /\ phase \in {"idle", "editor", "committed", "done"}
    /\ trailers \subseteq (Named \X (1..MaxVer))
    /\ unsafe \subseteq Sessions
    /\ badCommit \in BOOLEAN

(* [R-INT-1] [R-INT-2] Cleanup never deletes a record that is still needed: *)
(* no Open session, no archive newer than the commit's, and no session     *)
(* whose trailer group the human removed.                                  *)
NoNeededDeletion == unsafe = {}

(* An integration is committed with a current trailer group for each of    *)
(* its sessions. Not a rule: an accepted limit, so it must be VIOLATED     *)
(* (ProvIntegration.limit-incomplete-commit).                              *)
CommitIsComplete == ~badCommit

(* Reachability canaries: each must be VIOLATED.                           *)
NeverDeleted        == \A s \in Sessions : rec[s]
NeverCommitted      == phase # "done"
NeverResumedAtCommit == ~(phase = "committed" /\ \E s \in Named : st[s] = "open")
=============================================================================
