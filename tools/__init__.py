"""Operational tooling for the Lupus project.

Modules:
    runpod_status — read-only status of your RunPod account, GPU availability,
                    active pods, and network volumes.
    runpod_deploy — deploy a pod with auto-retry on capacity unavailable.
                    Uses the existing network volume by default.

These talk to the RunPod REST API at https://rest.runpod.io/v1.
Requires RUNPOD_API_KEY in .env (full access — used for read AND
deploy/terminate operations).
"""
