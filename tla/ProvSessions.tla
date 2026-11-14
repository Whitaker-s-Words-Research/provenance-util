--------------------------- MODULE ProvSessions ---------------------------
(***************************************************************************)
(* Session lifecycle and the detached archive process.                     *)
(*                                                                         *)
(* SPEC.md sections: "Session states", "Resuming", "Archive step",         *)
(* "Finishing" (--override), "Live state" (lock).                          *)
(* Rules: R-SESS-1 (any event reopens, new generation),                    *)
(*        R-SESS-2 (transitions are compare-and-set on the generation),    *)
(*        R-SESS-3 (archive waits for the agent process to exit, if prov   *)
(*                  knows its process ID).                                 *)
(*                                                                         *)
(* Ghost state: work[s] counts what the session has done (transcript       *)
(* growth). An archive "covers" the session when its snapshot = work[s].   *)
(* The gate integrates a session only when it is Archived, so Archived     *)
(* must mean "the archive covers everything the session did".              *)
(***************************************************************************)
(***************************************************************************)
(* Premises: what this model takes as facts about other tools. Each names *)
(* a row of "Appendix: test evidence" in SPEC.md, or says "untested".      *)
(*                                                                         *)
(* P1  prov knows the agent's process ID from CLAUDE_PID in the hook       *)
(*     environment, and that process is the agent. Tested: Claude Code     *)
(*     2.1.322, "CLAUDE_PID in the environment of a hook". PidKnown =      *)
(*     FALSE models an agent that has no such variable.                    *)
(* P2  The transcript does not grow after the agent process has exited.    *)
(*     Tested once: Claude Code 2.1.322, "The transcript of the live run". *)
(* P3  A resumed Claude Code session keeps its session ID. Tested: Claude  *)
(*     Code 2.1.322, "claude -p, then claude -p --resume <id>".            *)
(* P4  Every agent event of a live process reaches prov as a hook (Event), *)
(*     also after the human closed the session while it ran. Tested:       *)
(*     Claude Code 2.1.322, "prov session close during a live run".        *)
(* P5  The file lock of .git/provenance/lock is exclusive between          *)
(*     processes of one machine. Tested on Linux, also on a 9p mount;      *)
(*     not between the host and a VM.                                      *)
(***************************************************************************)
EXTENDS Integers, FiniteSets

CONSTANTS
    Sessions,
    MaxWork,        \* bound on work per session
    MaxGen,         \* bound on generations per session
    GenerationCheck, \* TRUE = R-SESS-2: an archive process writes its result
                    \*        only if the session is still Closing in its
                    \*        generation. FALSE = only if still Closing (the
                    \*        prov 0.1 code).
    WaitForExit,    \* TRUE = R-SESS-3; FALSE = archive snapshots immediately
    PidKnown        \* TRUE = prov knows the agent's process ID. Without it,
                    \*        R-SESS-3 cannot wait (accepted limit).

VARIABLES
    st,      \* "none" | "open" | "closing" | "archived" | "failed"
    gen,     \* live state: generation, bumped on every move into Open or Closing
    arch,    \* live state: work covered by the stored archive (-1 = none)
    alive,   \* ghost: the agent process exists
    work,    \* ghost: work done by the session
    procs    \* archive processes: [s, g, snap] (snap = -1 before the snapshot)

vars == <<st, gen, arch, alive, work, procs>>

Proc == [s : Sessions, g : 0..MaxGen, snap : -1..MaxWork]

Spawn(s, g) == procs \cup {[s |-> s, g |-> g, snap |-> -1]}
LiveProcFor(s) == \E p \in procs : p.s = s /\ p.g = gen[s]

(* The state as prov reads it. A Closing session with no live archive      *)
(* process of its generation is Failed: the 5-minute rule, calculated when *)
(* read, never stored (the model leaves out the 5 minutes). Nothing writes *)
(* it, so it needs no generation check.                                    *)
EffSt(s) == IF st[s] = "closing" /\ ~LiveProcFor(s) THEN "failed" ELSE st[s]

----------------------------------------------------------------------------
Init ==
    /\ st    = [s \in Sessions |-> "none"]
    /\ gen   = [s \in Sessions |-> 0]
    /\ arch  = [s \in Sessions |-> -1]
    /\ alive = [s \in Sessions |-> FALSE]
    /\ work  = [s \in Sessions |-> 0]
    /\ procs = {}

(* First event, or SessionStart of a resume. R-SESS-1: from any state.     *)
Start(s) ==
    /\ ~alive[s] /\ gen[s] < MaxGen
    /\ st'    = [st EXCEPT ![s] = "open"]
    /\ gen'   = [gen EXCEPT ![s] = @ + 1]
    /\ alive' = [alive EXCEPT ![s] = TRUE]
    /\ UNCHANGED <<arch, work, procs>>

(* Any other agent event (UserPromptSubmit, Stop) from a live process.     *)
(* R-SESS-1: a Closing/Archived/Failed session returns to Open.            *)
Event(s) ==
    /\ alive[s] /\ st[s] # "open" /\ gen[s] < MaxGen
    /\ st'  = [st EXCEPT ![s] = "open"]
    /\ gen' = [gen EXCEPT ![s] = @ + 1]
    /\ UNCHANGED <<arch, alive, work, procs>>

(* The agent does something. It does not ask prov first.                   *)
Work(s) ==
    /\ alive[s] /\ work[s] < MaxWork
    /\ work' = [work EXCEPT ![s] = @ + 1]
    /\ UNCHANGED <<st, gen, arch, alive, procs>>

(* SessionEnd: the agent process exits right after the hook.               *)
End(s) ==
    /\ alive[s] /\ st[s] = "open" /\ gen[s] < MaxGen
    /\ st'    = [st EXCEPT ![s] = "closing"]
    /\ gen'   = [gen EXCEPT ![s] = @ + 1]
    /\ alive' = [alive EXCEPT ![s] = FALSE]
    /\ procs' = Spawn(s, gen[s] + 1)
    /\ UNCHANGED <<arch, work>>

Crash(s) ==
    /\ alive[s]
    /\ alive' = [alive EXCEPT ![s] = FALSE]
    /\ UNCHANGED <<st, gen, arch, work, procs>>

(* prov session archive (or finish --override): Open -> Closing, the process *)
(* may still be alive. On a Failed session: run the archive step again.    *)
Close(s) ==
    /\ EffSt(s) \in {"open", "failed"} /\ gen[s] < MaxGen
    /\ st'    = [st EXCEPT ![s] = "closing"]
    /\ gen'   = [gen EXCEPT ![s] = @ + 1]
    /\ procs' = Spawn(s, gen[s] + 1)
    /\ UNCHANGED <<arch, alive, work>>

(* Archive process reads the transcript. R-SESS-3: only after the agent    *)
(* process has exited, if prov knows the process ID.                       *)
Snapshot(p) ==
    /\ p \in procs /\ p.snap = -1
    /\ (WaitForExit /\ PidKnown) => ~alive[p.s]
    /\ procs' = (procs \ {p}) \cup {[p EXCEPT !.snap = work[p.s]]}
    /\ UNCHANGED <<st, gen, arch, alive, work>>

(* R-SESS-2: the result is written only if the session is still Closing in *)
(* the same generation. A stale result is discarded.                       *)
Current(p) == st[p.s] = "closing" /\ gen[p.s] = p.g

MayWrite(p) == IF GenerationCheck THEN Current(p) ELSE st[p.s] = "closing"

Finish(p) ==
    /\ p \in procs /\ p.snap # -1
    /\ IF MayWrite(p)
       THEN /\ st'   = [st EXCEPT ![p.s] = "archived"]
            /\ arch' = [arch EXCEPT ![p.s] = p.snap]
       ELSE UNCHANGED <<st, arch>>
    /\ procs' = procs \ {p}
    /\ UNCHANGED <<gen, alive, work>>

Fail(p) ==
    /\ p \in procs
    /\ IF MayWrite(p)
       THEN st' = [st EXCEPT ![p.s] = "failed"]
       ELSE UNCHANGED st
    /\ procs' = procs \ {p}
    /\ UNCHANGED <<gen, arch, alive, work>>

(* The archive process dies without a result.                              *)
Die(p) ==
    /\ p \in procs
    /\ procs' = procs \ {p}
    /\ UNCHANGED <<st, gen, arch, alive, work>>


Next ==
    \/ \E s \in Sessions :
          Start(s) \/ Event(s) \/ Work(s) \/ End(s) \/ Crash(s) \/ Close(s)
    \/ \E p \in procs : Snapshot(p) \/ Finish(p) \/ Fail(p) \/ Die(p)

Spec == Init /\ [][Next]_vars

----------------------------------------------------------------------------
TypeOK ==
    /\ st \in [Sessions -> {"none", "open", "closing", "archived", "failed"}]
    /\ gen \in [Sessions -> 0..MaxGen]
    /\ arch \in [Sessions -> -1..MaxWork]
    /\ alive \in [Sessions -> BOOLEAN]
    /\ work \in [Sessions -> 0..MaxWork]
    /\ procs \subseteq Proc

(* [R-SESS-2] [R-SESS-3] Archived means the archive covers all the work,   *)
(* so the transcript hash in an integration covers every commit.           *)
ArchivedIsCurrent == \A s \in Sessions : st[s] = "archived" => arch[s] = work[s]

(* Reachability canaries: each must be VIOLATED.                           *)
NeverArchived      == \A s \in Sessions : st[s] # "archived"
NeverRearchived    == \A s \in Sessions : st[s] = "archived" => gen[s] <= 2
NeverStaleDiscard  == \A p \in procs : p.snap # -1 => Current(p)


----------------------------------------------------------------------------
(* Liveness. Fairness: an archive process that can take its next step      *)
(* takes it. A process can still die (Die); then the 5-minute rule makes   *)
(* the session Failed, as it is read (EffSt).                              *)
Fairness ==
    /\ WF_vars(\E p \in procs : Snapshot(p))
    /\ WF_vars(\E p \in procs : Finish(p) \/ Fail(p))

LiveSpec == Spec /\ Fairness

(* A Closing session whose agent has exited does not stay Closing: it      *)
(* becomes Archived or Failed, or its agent runs again (a resume).         *)
ClosingEnds ==
    \A s \in Sessions : (EffSt(s) = "closing" /\ ~alive[s]) ~> (EffSt(s) # "closing" \/ alive[s])
=============================================================================
