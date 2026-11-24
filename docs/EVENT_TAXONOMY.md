# Message-derived event taxonomy — 12 September 2026

Relic observes messages and their supported event claims. A type describes what is alleged to have happened, not whether it is verified, whether the message is useful, or whether it has coordinates. This review replaces the narrow eleven-type draft with **30 stable types in seven browsing groups**. `web/src/event-taxonomy.json` is the readable vocabulary and definitions; Rust validates the same identifiers. Existing eleven identifiers and old citations remain valid.

## Manual review and findings

Reviewed a systematic every-third-message sample from the 154-message retained publication, with closer original-text reads for diplomacy, territorial control, maritime interdiction, sanctions and judicial reporting. The private review ledger records the 49 identifiable excerpts actually inspected; some source posts are themselves truncated. This is qualitative vocabulary design, not an accuracy score or a claim to have reviewed every message. Real source material stays in the private workspace; acceptance fixtures are fictional.

The sample contains much more than strikes: reports of territorial capture; patrols and deployments; vessel diversions under a blockade; alerts and all-clears; postponed negotiations and scheduled meetings; sanctions; administrative travel restrictions; judicial sentences; retrospective footage and satellite imagery; and substantial commentary, promotions and messages without text. None of these should be shoehorned into Strike. The old implementation had eleven types, but very few live messages had reached that new recipe before the billing guard stopped the upgrade.

| Group | Stable types |
| --- | --- |
| Conflict | Strike, armed attack, air defence, weapons launch, territorial change |
| Military activity | Deployment, military activity, maritime interdiction |
| Safety | Explosion, security alert, all clear, evacuation, rescue, fire, accident, natural hazard |
| Infrastructure | Infrastructure disruption, structural damage, transport disruption, cyber incident |
| Politics and diplomacy | Diplomatic contact, agreement, sanctions, policy change |
| Civil affairs | Protest, arrest/detention, legal proceeding, election, aid operation |
| Other | Other concrete event |

Rescue, aid, elections, cyber incidents and natural hazards extend the observed vocabulary for adjacent source coverage; they were not all present in the reviewed sample. They require their own evaluation cases and are not claims about the sampled messages.

## Distinctions that matter

- A blast does not establish a strike or a weapon. Aircraft activity does not establish an attack; a civil aircraft observation must not become military activity.
- Capture/loss of control differs from deployment. Maritime interdiction differs from an attack. A cumulative vessel-diversion total is one aggregate report, not that many independent incidents.
- A warning and an all-clear are concrete developments, but neither proves an attack. Meeting plans stay planned; postponements must not become completed meetings.
- A sanction, decision or court sentence is a concrete change; ordinary rhetoric, political opinion, reviews, greetings and promotions stay outside Events. A statement can contain event claims without making every statement an event.
- The event's state remains reported/denied/planned/historical/unclear. Publication time is not occurrence time. Newly posted footage and historical imagery must preserve uncertainty about when the depicted event happened.
- Weapon/platform (aircraft, drone, missile, artillery), target (vehicle, ship, power facility), impact (casualties/damage), actors, quantities and subtype are separate dimensions. Do not multiply event types for every combination or turn each attribute into another event. The current protocol retains them in supported text; structured attribute extraction is a subsequent evaluated addition.
- A translated copy or quoted relay is not independent confirmation. Message, claim and eventual incident identity remain distinct. Grouping is still pending.

## Implementation and release gate — 20 September 2026

Prompt `relic.message-events.v8`, recipe `message-events-v6`, retain 30 controlled types. Structural damage covers a damaged building/bridge without inventing its cause or treating damage as a service outage. The grouped selector remains the only category control.

The 49-case fictional suite has a reviewed 49/49 offline replay of retained DeepSeek responses. The fresh run scored 48/49 before refining one evaluation criterion: ancillary unknown-cause language may remain in the complete reading instead of becoming an additional extracted claim. Gold corrections distinguish routine civilian travel and denied damage. These cases were reused during development; this is not independent held-out accuracy or proof of live precision. Original responses and the initial score are retained.

The evaluated gate is active. Live messages are upgrading in bounded batches; unsupported quotations and invalid event/place links still reject. A redundant English translation field is discarded in favor of the original, with the discarded provider response preserved. This compatibility recovery never repairs factual quotations or invents locations. Recovered paid responses reuse their exact receipt; no second model call is needed.

Next: complete eligible text processing within daily limits, review live rejections and unseen multilingual cases, then implement evidence-backed event grouping and geographic area/multi-site display. Geography re-resolution is independently versioned and does not count as a semantic recipe upgrade.
