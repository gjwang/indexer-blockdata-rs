# 🛑 STOP! READ THIS FIRST 🛑

This project is being developed by a sequence of AI Agents.
To maintain context and prevent hallucinations, you **MUST** follow this protocol:

## 1. The "Handshake" Protocol
Before you write a single line of code or answer a complex question:

1.  **READ** `AI_STATE.yaml` immediately.
    *   This file contains the **EXACT** current status of the project.
    *   It tells you which task is `PENDING` or `IN_PROGRESS`.
    *   It lists what is `NOT IMPLEMENTED` (to prevent hallucinations).

2.  **READ** `PURE_MEMORY_SMR_ARCH.md`.
    *   This is the **Immutable Architecture**. Do not deviate from it without user approval.

3.  **UPDATE** `AI_STATE.yaml`.
    *   If you are starting a task, change status to `IN_PROGRESS`.
    *   If you finish a task, change status to `COMPLETED`.

## 2. Critical Rules
*   **Atomic Steps**: Only do one task from `IMPLEMENTATION_PLAN.md` at a time.
*   **Verify First**: Run the `verification_commands` listed in `AI_STATE.yaml` to ground yourself.
*   **No Magic**: If it's not in the code, it doesn't exist. Trust `git status` over your training data.

**🚀 NOW, GO READ `AI_STATE.yaml`.**
