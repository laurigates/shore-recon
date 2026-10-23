# 0001. Record architecture decisions

- Status: accepted
- Date: 2026-10-23

## Context

Backend choices (segmentation model, SfM, dense, splat) and mask handling
depend on spike results (M0) and licence checks. These need a durable record.

## Decision

Record decisions as ADRs in `docs/adr/`, numbered sequentially, using
`0000-template.md`. Superseded ADRs stay in place with an updated status.

## Consequences

Each backend or format decision made in M0 and later gets an ADR before the
corresponding implementation is merged.
