# Eval evl_01M37SPK06RMB0MEPM4SBMD22H: incomplete, 0.67 agreed

Subject: worktree 5f50b44460b7 · 2026-09-23T18:56:47.518Z
Judged: 3 open Asks
- ask_01M37R118QDAM8MNV2W9SRBSWA Account erasure plan — .fab7/rf/asks/ask_01M37R118QDAM8MNV2W9SRBSWA/prompt.txt
- ask_01M37RDYJCHWWSD12A5DGEYE08 Build account erasure — .fab7/rf/asks/ask_01M37RDYJCHWWSD12A5DGEYE08/prompt.txt
- ask_01M37SHXFXH3N7V95EQAH8JYGV Fix erasure Eval findings — .fab7/rf/asks/ask_01M37SHXFXH3N7V95EQAH8JYGV/prompt.txt

## Unresolved
- i15 Read plan.md, adr/ and spec/ in plans/account-erasure/ first. (from ask_01M37RDYJCHWWSD12A5DGEYE08)
  - coverage unknown: Reading order is a process step and cannot be seen in files. The only trace is the src/erasure.js comment naming plans/account-erasure/spec/account-erasure.md, and code that follows that spec's order.
  - drift unknown: Reading the plan first is a process step; src/erasure.js cites the spec and ADRs in comments and the code follows the spec order, but the repository cannot show the order in which files were read.
  - adversary yes: This is a process item. The evidence is circumstantial but strong: src/erasure.js:4-5 cites spec/account-erasure.md, and lines 7-18 follow the spec's 9-step order exactly, including the kinds list and rule strings from ADR 0003.
- i17 Done when every step's done condition holds and npm test passes. (from ask_01M37RDYJCHWWSD12A5DGEYE08)
  - coverage unknown: The done conditions appear to be met in code and tests. Step 1 is covered by the first three tests in test/erasure.test.js, step 2 by the 'DELETE /me erases every copy' test, step 3 by the three retention tests in test/jobs.test.js. Whether npm test passes cannot be established without running it, and running it is forbidden here.
  - drift unknown: By reading, each step's done condition is covered by tests in test/erasure.test.js and test/jobs.test.js, and test/accounts.test.js and test/server.test.js look compatible, but running npm test is forbidden, so passing cannot be established.
  - adversary yes: I did not run the tests. I traced every test by hand: accounts, server, invoices, erasure (4) and jobs (4). None fails. The id sequencing (usr_1, inv_2...) makes the substring checks sound, the listening handler registered in createServer fires before the listen callback, and each step's done condition is asserted.
- i21 Gall: get step 1's thin path (erase the account record, strip invoices, audit entry) passing end to end before extending to the other stores. (from ask_01M37RDYJCHWWSD12A5DGEYE08)
  - coverage unknown: Build order is process. The test file's structure matches the order (step 1 thin-path tests come before the step 2 DELETE /me test), but the files cannot show that step 1 passed before step 2 was started.
  - drift unknown: Build order is process. Test structure shows separate step-1 tests (record/sessions, invoice minimisation, single audit entry) before the step-2 DELETE /me test, but the worktree cannot show that step 1 passed before step 2 was built.
  - adversary unknown: The test layout matches a thin step 1 (erasure.test.js:29-64) followed by step 2 (line 66). Whether step 1 passed end to end before step 2 was built cannot be seen in the diff.
- i34 Done when both are fixed and npm test passes. (from ask_01M37SHXFXH3N7V95EQAH8JYGV)
  - coverage unknown: Both fixes are present in files (i32 in src/server.js and src/jobs.js, i33 in test/erasure.test.js). Whether npm test passes cannot be established without running it.
  - drift unknown: Both fixes are present in the code (i32, i33), but npm test cannot be run under the assessor's restrictions, so passing is not established.
  - adversary yes: Both fixes are present: scheduling (server.js:57-59) and the second-account test (erasure.test.js:66-104). I traced npm test to pass but did not run it.

## Limitations
- the verdict is a judgement by 3 sub-agents; agreement is its confidence, nothing here is certain
- RingFrame ran none of the project's commands; commands_run in a judgement is that judge's own report
- 10 unrecorded prompts followed the open Asks, and 0 more came between the anchor and the first of them; unexplained changes may follow either
