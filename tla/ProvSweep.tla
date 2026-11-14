---------------------------- MODULE ProvSweep ----------------------------
(***************************************************************************)
(* Sweep attribution in ONE agent worktree.                                *)
(*                                                                         *)
(* SPEC.md sections: "Binding rules", "Keeping attribution complete",      *)
(* "Git hooks" (pre-commit sweep, contended refusal).                      *)
(* Rules: R-SWEEP-1 (occupant), R-SWEEP-2 (taint),                         *)
(*        R-SWEEP-3 (contended refusal), R-SWEEP-4 (bg limit),             *)
(*        R-SWEEP-5 (sweep at SessionEnd).                                 *)
(*                                                                         *)
(* Scope: only changes that NO checkpoint covers (Bash output, generators, *)
(* background processes). Edits through the agent's own edit tools are    *)
(* checkpointed by git-ai's hooks and are not modeled.                     *)
(*                                                                         *)
(* Ghost state: dirty[f] = the TRUE writers of changes to file f that have *)
(* no checkpoint. prov never sees dirty's contents, only which files are   *)
(* changed (git status).                                                   *)
(*                                                                         *)
(* Taint is a checkpoint, not a missing one. A change with no checkpoint   *)
(* is not doubtful by itself: in a test, git-ai gave a file with no        *)
(* checkpoint to the AI session that had other lines in the same commit.   *)
(* So when the occupant changes, prov gives the changes of that moment a   *)
(* known_human checkpoint. In an agent commit, the gate treats such lines  *)
(* as doubtful.                                                            *)
(***************************************************************************)
(***************************************************************************)
(* Premises: what this model takes as facts about other tools. Each names *)
(* a row of "Appendix: test evidence" in SPEC.md, or says "untested".      *)
(*                                                                         *)
(* P1  An edit through the agent's own edit tools gets a checkpoint from   *)
(*     git-ai's hooks, so it is not part of `dirty`. Tested: Claude Code   *)
(*     2.1.322, "A session edits one file (own git-ai hooks)".             *)
(* P2  A change with no checkpoint, in an agent commit with lines of an AI *)
(*     session, goes to that session. The model is kinder: it calls such a *)
(*     change Doubtful in Commit. Tested: git-ai 1.7.6, "A file with no    *)
(*     checkpoint and a file with an ai_agent checkpoint".                 *)
(* P3  An ai_agent checkpoint under the tool prov-unattributed with a new  *)
(*     conversation ID keeps its own session in the note, also in an agent *)
(*     commit with lines of another session; the gate lists its lines.     *)
(*     This is the Doubtful of a taint. Tested: git-ai 1.7.6, "The same,   *)
(*     with an ai_agent checkpoint (tool prov-unattributed ...)".          *)
(* P4  A checkpoint covers only the changes since the checkpoint before it *)
(*     on that file, so a sweep or a taint covers `dirty` and nothing      *)
(*     older. Tested: git-ai 1.7.6, "Replay with known_human and agent-v1  *)
(*     stages".                                                            *)
(* P5  A checkpoint of type human just before an ai_agent checkpoint makes *)
(*     the ai_agent checkpoint record nothing. The sweep and the taint     *)
(*     never do that. Tested: git-ai 1.7.6.                                *)
(* P6  A process that the agent started can write after SessionEnd. Not a  *)
(*     test: the BackgroundWrites switch models it as an accepted limit.   *)
(* P7  A checkpoint in pre-commit is applied before git records the commit *)
(*     (Commit takes the sweep's credits at once). git-ai applies a        *)
(*     checkpoint after the command returns; prov runs git-ai await at the *)
(*     end of pre-commit. Tested: git-ai 1.7.6, "The full test suite in a  *)
(*     Podman container". Without the wait, a credit can become a session  *)
(*     that git-ai invents: Doubtful at the gate, so SweepSound still      *)
(*     holds; only the attribution is lost.                                *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    Sessions,          \* agent sessions that may bind to this worktree
    Files,             \* abstract files of the worktree
    TaintEnabled,      \* TRUE = R-SWEEP-2; FALSE = SPEC.md 0.1 rule
    BackgroundWrites,  \* TRUE = a session's leftover processes keep writing
                       \*        after it ends or crashes (R-SWEEP-4 limit)
    None, Doubtful     \* model values

VARIABLES
    st,        \* session state as prov records it: "none" | "open" | "closed"
    alive,     \* ghost: the agent process exists
    dirty,     \* ghost: [Files -> SUBSET Sessions], true writers of changes with no checkpoint
    owner,     \* live state: the session prov last saw as the sole occupant
    credited,  \* history: <<true writer, credited session or Doubtful>>
    tainted    \* history: a taint checkpoint covered a change

vars == <<st, alive, dirty, owner, credited, tainted>>

----------------------------------------------------------------------------
(* Occupants: Open and not stale. Stale = the process is gone, or no event *)
(* for 24 hours. For the sweep, a session that went idle is the same as   *)
(* one that crashed and is resumed later (Crash, then Start): it writes   *)
(* nothing meanwhile and stops occupying. So `alive` stands for both.     *)
OccOf(s_, a_) == {s \in Sessions : s_[s] = "open" /\ a_[s]}
Occupants == OccOf(st, alive)

DirtyFiles == {f \in Files : dirty[f] # {}}
Writers == UNION {dirty[f] : f \in Files}

Sole(occ) == IF Cardinality(occ) = 1 THEN CHOOSE x \in occ : TRUE ELSE None

(***************************************************************************)
(* Observe(occ): run by EVERY prov hook in this worktree, under the lock,  *)
(* with the occupancy that the hook computes. R-SWEEP-2: when occupancy is *)
(* not exactly {owner}, every change that has no checkpoint now gets one   *)
(* that names no session (Doubtful), and owner becomes the sole occupant   *)
(* (or None).                                                              *)
(***************************************************************************)
Mismatch(occ) == ~(owner \in Sessions /\ occ = {owner})
Taints(occ)   == TaintEnabled /\ Mismatch(occ)
NewOwner(occ) == IF Mismatch(occ) THEN Sole(occ) ELSE owner

(* The credits of one hook. With `sweeps`, the hook also sweeps for the    *)
(* sole occupant what the taint left.                                      *)
Credits(occ, sweeps) ==
    IF Taints(occ) THEN {<<w, Doubtful>> : w \in Writers}
    ELSE IF sweeps /\ Sole(occ) \in Sessions THEN {<<w, Sole(occ)>> : w \in Writers}
    ELSE {}
Covers(occ, sweeps) == Taints(occ) \/ (sweeps /\ Sole(occ) \in Sessions)

Observe(occ, sweeps) ==
    /\ owner'    = NewOwner(occ)
    /\ credited' = credited \cup Credits(occ, sweeps)
    /\ dirty'    = IF Covers(occ, sweeps) THEN [f \in Files |-> {}] ELSE dirty
    /\ tainted'  = (tainted \/ (Taints(occ) /\ Writers # {}))

(* A hook that observes and then sweeps (Stop, SessionEnd, pre-commit).    *)
ObserveAndSweep(occ) == Observe(occ, TRUE)
(* A hook that only observes (SessionStart).                                *)
ObserveOnly(occ) == Observe(occ, FALSE)

----------------------------------------------------------------------------
Init ==
    /\ st       = [s \in Sessions |-> "none"]
    /\ alive    = [s \in Sessions |-> FALSE]
    /\ dirty    = [f \in Files |-> {}]
    /\ owner    = None
    /\ credited = {}
    /\ tainted  = FALSE

(* SessionStart: a new session, or a resume of an ended, crashed or idle   *)
(* one.                                                                    *)
Start(s) ==
    /\ ~alive[s]
    /\ LET nst == [st EXCEPT ![s] = "open"]
           nal == [alive EXCEPT ![s] = TRUE]
       IN /\ st' = nst /\ alive' = nal
          /\ ObserveOnly(OccOf(nst, nal))

(* Stop: end of a turn; prov sweeps.                                       *)
Stop(s) ==
    /\ st[s] = "open" /\ alive[s]
    /\ ObserveAndSweep(Occupants)
    /\ UNCHANGED <<st, alive>>

(* SessionEnd. R-SWEEP-5: the hook first observes and sweeps with the     *)
(* occupancy before the session leaves, then records the end. Then it      *)
(* observes again with the occupancy after the end (R-SWEEP-2, owner       *)
(* decision 2026-11-14): if one session is left, it becomes the owner now, *)
(* so its next hook does not taint its own later work. The end sweep is    *)
(* for complete attribution; SweepSound holds without it, so it has no     *)
(* switch here.                                                            *)
End(s) ==
    /\ st[s] = "open" /\ alive[s]
    /\ LET nst   == [st EXCEPT ![s] = "closed"]
           nal   == [alive EXCEPT ![s] = FALSE]
           occ1  == Occupants
           occ2  == OccOf(nst, nal)
           own1  == NewOwner(occ1)
           cov1  == Covers(occ1, TRUE)
           left  == IF cov1 THEN {} ELSE Writers
           mis2  == ~(own1 \in Sessions /\ occ2 = {own1})
           taint2 == TaintEnabled /\ mis2
       IN /\ st' = nst /\ alive' = nal
          /\ owner'    = IF mis2 THEN Sole(occ2) ELSE own1
          /\ credited' = credited \cup Credits(occ1, TRUE)
                          \cup (IF taint2 THEN {<<w, Doubtful>> : w \in left} ELSE {})
          /\ dirty'    = IF cov1 \/ taint2 THEN [f \in Files |-> {}] ELSE dirty
          /\ tainted'  = (tainted \/ (Taints(occ1) /\ Writers # {}) \/ (taint2 /\ left # {}))

(* The process exits with no event, or the session goes idle for 24 hours. *)
(* No hook runs: prov learns of it only at the next hook.                  *)
Crash(s) ==
    /\ alive[s]
    /\ alive' = [alive EXCEPT ![s] = FALSE]
    /\ UNCHANGED <<st, dirty, owner, credited, tainted>>

(* An uncheckpointed write. A live, active session writes during a turn.  *)
(* With BackgroundWrites, a process it left behind can write at any time. *)
Write(s, f) ==
    /\ \/ alive[s] /\ st[s] = "open"
       \/ BackgroundWrites /\ st[s] # "none"
    /\ dirty' = [dirty EXCEPT ![f] = @ \cup {s}]
    /\ UNCHANGED <<st, alive, owner, credited, tainted>>

(* git commit in this worktree. pre-commit refuses in a contended          *)
(* worktree (R-SWEEP-3), else observes and sweeps. A change that still has *)
(* no checkpoint is committed without one. The model calls that Doubtful,  *)
(* which is the best case: git-ai can give it to a session of the commit.  *)
Commit ==
    /\ Cardinality(Occupants) <= 1
    /\ DirtyFiles # {}
    /\ LET occ == Occupants
       IN /\ credited' = credited \cup Credits(occ, TRUE)
                         \cup (IF Covers(occ, TRUE) THEN {} ELSE {<<w, Doubtful>> : w \in Writers})
          /\ dirty'    = [f \in Files |-> {}]
          /\ owner'    = NewOwner(occ)
          /\ tainted'  = (tainted \/ (Taints(occ) /\ Writers # {}))
    /\ UNCHANGED <<st, alive>>

Next ==
    \/ Commit
    \/ \E s \in Sessions :
          \/ Start(s) \/ Stop(s) \/ End(s) \/ Crash(s)
          \/ \E f \in Files : Write(s, f)

Spec == Init /\ [][Next]_vars

----------------------------------------------------------------------------
TypeOK ==
    /\ st \in [Sessions -> {"none", "open", "closed"}]
    /\ alive \in [Sessions -> BOOLEAN]
    /\ dirty \in [Files -> SUBSET Sessions]
    /\ tainted \in BOOLEAN
    /\ owner \in Sessions \cup {None}
    /\ credited \subseteq (Sessions \X (Sessions \cup {Doubtful}))

(* [R-SWEEP-2] A change is credited only to the session that made it.      *)
(* Anything prov cannot attribute goes to the gate as doubtful.            *)
SweepSound == \A p \in credited : p[2] \in {p[1], Doubtful}

(* Reachability canaries: each must be VIOLATED, proving that the model    *)
(* really sweeps, really taints, and really reaches contention.           *)
NeverCredited  == \A p \in credited : p[2] = Doubtful
NeverTainted   == ~tainted
NeverContended == Cardinality(Occupants) <= 1
=============================================================================
