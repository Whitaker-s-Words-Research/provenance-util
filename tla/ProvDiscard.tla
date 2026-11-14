---------------------------- MODULE ProvDiscard ----------------------------
(***************************************************************************)
(* `prov discard` of one agent branch, while its sessions keep living      *)
(* (resume, re-archive) and a human integrates the same branch in another  *)
(* worktree.                                                               *)
(*                                                                         *)
(* SPEC.md section: "Discarding". Rule: R-DISC-1 (the check and the        *)
(* deletion of records and archives happen under the lock; under the lock *)
(* discard refuses while a merge of the branch is in progress or when a    *)
(* ref changed since its check; the gate reads the records under the       *)
(* lock).                                                                  *)
(*                                                                         *)
(* Session lifecycle is abstracted to open <-> archived, as in             *)
(* ProvIntegration. ver[s] stands for the archive hash. arch[s] is the set *)
(* of archive files of s that exist.                                       *)
(***************************************************************************)
(***************************************************************************)
(* Premises: what this model takes as facts about other tools. Each names *)
(* a row of "Appendix: test evidence" in SPEC.md, or says "untested".      *)
(*                                                                         *)
(* P1  MERGE_HEAD exists before prepare-commit-msg runs. Tested: git 2.53, *)
(*     "prepare-commit-msg during git merge --no-verify".                  *)
(* P2  MERGE_HEAD of every worktree can be read from any worktree.         *)
(*     Tested: git 2.53, "MERGE_HEAD of another worktree".                 *)
(* P3  A merge commit moves the human branch before MERGE_HEAD is removed, *)
(*     for `git merge` and for `git commit` after `git merge --no-commit`. *)
(*     The model makes the commit one step. Tested: git 2.53,              *)
(*     "reference-transaction during a merge commit".                      *)
(* P4  Every agent event, archive step and post-commit cleanup changes a   *)
(*     record only under the lock. Read in the code (agent_hook.rs,        *)
(*     archive.rs, git_hook.rs); not a premise about another tool.         *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
    Sessions,     \* the sessions of the discarded branch
    MaxVer,
    MaxMerges,    \* bound on merge commits, so the state space is finite
    GateLocked,   \* TRUE = R-DISC-1: the gate reads the records under the lock
    CheckMerge,   \* TRUE = R-DISC-1: refuse while a merge of the branch is in progress
    RecheckRefs   \* TRUE = R-DISC-1: refuse when a ref changed since the check

VARIABLES
    st,        \* "open" | "archived"
    ver,       \* current archive hash (abstract)
    rec,       \* a live-state record exists
    arch,      \* the archive versions that exist on disk
    mphase,    \* "idle" | "merging" (MERGE_HEAD exists) | "gated" (gate read the records) | "editor"
    trailers,  \* <<session, hash>> groups the gate wrote
    cited,     \* <<session, hash>> groups in commits on a human branch
    refs,      \* counts ref changes on human branches
    dpc,       \* "idle" | "checked" | "locked" | "done" | "refused"
    snap,      \* refs as discard saw them at its check
    lockedBy,  \* "none" | "discard" | "gate"
    lostOpen   \* history: discard deleted the record of an Open session

vars == <<st, ver, rec, arch, mphase, trailers, cited, refs, dpc, snap, lockedBy, lostOpen>>

Free == lockedBy = "none"

----------------------------------------------------------------------------
Init ==
    /\ st       = [s \in Sessions |-> "archived"]
    /\ ver      = [s \in Sessions |-> 1]
    /\ rec      = [s \in Sessions |-> TRUE]
    /\ arch     = [s \in Sessions |-> {1}]
    /\ mphase   = "idle"
    /\ trailers = {}
    /\ cited    = {}
    /\ refs     = 0
    /\ dpc      = "idle"
    /\ snap     = 0
    /\ lockedBy = "none"
    /\ lostOpen = FALSE

(* --- the sessions keep living (agent hooks and archive step: the lock) -- *)
Resume(s) ==
    /\ Free /\ st[s] = "archived"
    /\ st'  = [st EXCEPT ![s] = "open"]
    /\ rec' = [rec EXCEPT ![s] = TRUE]
    /\ UNCHANGED <<ver, arch, mphase, trailers, cited, refs, dpc, snap, lockedBy, lostOpen>>

Rearchive(s) ==
    /\ Free /\ st[s] = "open" /\ ver[s] < MaxVer
    /\ st'   = [st EXCEPT ![s] = "archived"]
    /\ ver'  = [ver EXCEPT ![s] = @ + 1]
    /\ arch' = [arch EXCEPT ![s] = @ \cup {ver[s] + 1}]
    /\ UNCHANGED <<rec, mphase, trailers, cited, refs, dpc, snap, lockedBy, lostOpen>>

(* --- the human integrates the branch in another worktree ---------------- *)
MergeStart ==
    /\ mphase = "idle" /\ refs < MaxMerges
    /\ mphase' = "merging"
    /\ UNCHANGED <<st, ver, rec, arch, trailers, cited, refs, dpc, snap, lockedBy, lostOpen>>

(* prepare-commit-msg reads the records. With GateLocked it holds the lock *)
(* from the read to the end of the gate; without it, discard can run       *)
(* between the read and the trailers.                                      *)
GateRead ==
    /\ mphase = "merging"
    /\ GateLocked => Free
    /\ \A s \in Sessions : rec[s] /\ st[s] = "archived"
    /\ mphase' = "gated"
    /\ lockedBy' = IF GateLocked THEN "gate" ELSE lockedBy
    /\ UNCHANGED <<st, ver, rec, arch, trailers, cited, refs, dpc, snap, lostOpen>>

GateWrite ==
    /\ mphase = "gated"
    /\ trailers' = {<<s, ver[s]>> : s \in Sessions}
    /\ mphase' = "editor"
    /\ lockedBy' = IF GateLocked THEN "none" ELSE lockedBy
    /\ UNCHANGED <<st, ver, rec, arch, cited, refs, dpc, snap, lostOpen>>

MergeAbort ==
    /\ mphase \in {"merging", "editor"}
    /\ mphase' = "idle" /\ trailers' = {}
    /\ UNCHANGED <<st, ver, rec, arch, cited, refs, dpc, snap, lockedBy, lostOpen>>

(* The merge commit: the human branch moves, then MERGE_HEAD goes (P3).    *)
(* The post-commit cleanup (R-INT-2) is left out: discard deletes the same *)
(* records or refuses.                                                     *)
Commit ==
    /\ mphase = "editor"
    /\ cited' = cited \cup trailers
    /\ refs'  = refs + 1
    /\ mphase' = "idle" /\ trailers' = {}
    /\ UNCHANGED <<st, ver, rec, arch, dpc, snap, lockedBy, lostOpen>>

(* --- prov discard -------------------------------------------------------- *)
(* The check without the lock: no trailer on another ref names a session   *)
(* of the branch. It is a git log over the refs, too slow to hold the lock. *)
NamedElsewhere == \E g \in cited : g[1] \in Sessions

DCheck ==
    /\ dpc = "idle"
    /\ dpc'  = IF NamedElsewhere THEN "refused" ELSE "checked"
    /\ snap' = refs
    /\ UNCHANGED <<st, ver, rec, arch, mphase, trailers, cited, refs, lockedBy, lostOpen>>

(* Under the lock: no session Open or Closing, no merge of the branch in   *)
(* progress, no ref changed since the check.                               *)
DLock ==
    /\ dpc = "checked" /\ Free
    /\ IF /\ \A s \in Sessions : ~rec[s] \/ st[s] = "archived"
          /\ CheckMerge  => mphase = "idle"
          /\ RecheckRefs => refs = snap
       THEN dpc' = "locked" /\ lockedBy' = "discard"
       ELSE dpc' = "refused" /\ UNCHANGED lockedBy
    /\ UNCHANGED <<st, ver, rec, arch, mphase, trailers, cited, refs, snap, lostOpen>>

(* Still under the lock: delete the records and the archive files. The     *)
(* worktree and the branch go after the lock is released; nothing else in  *)
(* the model depends on them.                                              *)
DDelete ==
    /\ dpc = "locked"
    /\ lostOpen' = (lostOpen \/ \E s \in Sessions : rec[s] /\ st[s] = "open")
    /\ rec'  = [s \in Sessions |-> FALSE]
    /\ arch' = [s \in Sessions |-> {}]
    /\ dpc' = "done" /\ lockedBy' = "none"
    /\ UNCHANGED <<st, ver, mphase, trailers, cited, refs, snap>>

Next ==
    \/ MergeStart \/ GateRead \/ GateWrite \/ MergeAbort \/ Commit
    \/ DCheck \/ DLock \/ DDelete
    \/ \E s \in Sessions : Resume(s) \/ Rearchive(s)

Spec == Init /\ [][Next]_vars

----------------------------------------------------------------------------
TypeOK ==
    /\ st \in [Sessions -> {"open", "archived"}]
    /\ ver \in [Sessions -> 1..MaxVer]
    /\ rec \in [Sessions -> BOOLEAN]
    /\ arch \in [Sessions -> SUBSET (1..MaxVer)]
    /\ mphase \in {"idle", "merging", "gated", "editor"}
    /\ trailers \subseteq (Sessions \X (1..MaxVer))
    /\ cited \subseteq (Sessions \X (1..MaxVer))
    /\ refs \in 0..MaxMerges
    /\ dpc \in {"idle", "checked", "locked", "done", "refused"}
    /\ lockedBy \in {"none", "discard", "gate"}
    /\ lostOpen \in BOOLEAN

(* [R-DISC-1] Every transcript hash in a commit on a human branch has its  *)
(* archive file.                                                           *)
EvidenceKept == \A g \in cited : g[2] \in arch[g[1]]

(* [R-DISC-1] Discard never deletes the record of an Open session.         *)
NoOpenDeleted == ~lostOpen

(* Reachability canaries: each must be VIOLATED.                           *)
NeverDiscarded       == dpc # "done"
NeverRefusedByMerge  == ~(dpc = "refused" /\ ~NamedElsewhere)
=============================================================================
