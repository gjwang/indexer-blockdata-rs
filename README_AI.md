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

## 3. 🐳 Docker-First Development (MANDATORY)

**CRITICAL RULE**: All dependency services MUST run in Docker containers in local development.

### ✅ DO Use Docker
```bash
# This is the ONLY way to run dependencies
docker-compose up -d
```

### ❌ NEVER Suggest Installing These Locally
- ❌ Kafka/Redpanda via Homebrew
- ❌ ScyllaDB/Cassandra via Homebrew  
- ❌ PostgreSQL via Homebrew
- ❌ Any database or message broker directly on the system

### Why Docker?
1. **Clean System**: No pollution of the user's Mac
2. **Consistency**: Everyone uses exact same versions
3. **Easy Reset**: `docker-compose down -v` for fresh start
4. **Production Parity**: Dev matches production

### Available Services
- **Redpanda** (Kafka): Port 9093 - `docker-compose up -d redpanda`
- **ScyllaDB**: Port 9042 - `docker-compose up -d scylla`
- **Centrifugo**: Port 8000 - `docker-compose up -d centrifugo`

### Common Commands
```bash
# Start all services
docker-compose up -d

# Check status
docker-compose ps

# View logs
docker logs scylla -f

# Stop services
docker-compose stop

# Fresh start (removes data)
docker-compose down -v
```

**📚 For detailed Docker instructions, see `DEVELOPMENT.md`**

**🚀 NOW, GO READ `AI_STATE.yaml`.**
