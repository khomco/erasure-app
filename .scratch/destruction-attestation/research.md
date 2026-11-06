# Destruction records: what the standards require (research, 2026-11-05)

Primary sources read: NIST SP 800-88 Rev. 2 (https://nvlpubs.nist.gov/nistpubs/SpecialPublications/NIST.SP.800-88r2.pdf),
the NAID AAA Specifications Reference Manual 1026O (linked from https://isigmaonline.org/certifications/naid-aaa-certification/),
and the e-Stewards Standard V4.1 (https://e-stewards.org/wp-content/uploads/2022/02/e-Stewards-Standard-V4.1.pdf).
R2v3 comes from SERI's knowledge base (the standard itself was not retrieved). IEEE 2883-2022 and ADISA are
**secondary sources only** because of a paywall and access blocks.

## NIST SP 800-88 Rev. 2
- §4.6: "a certificate of sanitization … should be completed for each ISM that has been sanitized". It may be electronic, and the person "might simply … scan each bar code as the ISM is sanitized". The requirement is a *should*, applied per device.
- Fields: manufacturer, model, serial, property number, media type and source, method (clear/purge/destroy), technique, tool and version, verification method. For the people involved: name, position/title, date, location, contact, signature.
- §4.5.1: verifying a Destroy means inspecting the remnants and identifying the equipment used. §4.5.2: validation is a separate accept/reject decision.
- Appendix C sample form: "Person Performing Sanitization", Verification, Validation, and an **optional "Concurrence"** block. Signature statement: "I attest that the information provided on this statement is accurate to the best of my knowledge."
- §3.1.3: the Destroy techniques are disintegrate, incinerate, melt, pulverize and shred. Bending, cutting and drilling "may only partly damage the ISM". Degaussing "does not currently constitute a destroy sanitization technique". Pulverize and shred "should be avoided for anything but the lowest security categories".

## NAID AAA (i-SIGMA)
- §4.3 (hard drives): a written process description is given to the Data Controller "prior to destruction". "Serial numbers of all hard drives … being destroyed … are recorded" unless the customer opts out, and the log is returned "upon the completion of the service". The pass criterion is that the drive is damaged until "the platters will not engage". §4.4 (SSDs): serials are recorded "where possible".
- For physical destruction, the required artifact is the **serial log**. "Certificate of Destruction" appears only in the overwrite and degauss endorsements (§4.6, §4.7).
- §2.5: CCTV must capture everything from entry through destruction, kept 90 days. §4.13: destruction within 30 days. §4.20: defined quality control. §1.15: electronic receipts are acceptable with "a verifiable electronic audit trail". Staff are screened and badged.
- Witnessing is only an option the provider must allow (§5.3).

## R2v3 (SERI; secondary for detail)
- Core 7(c)(2)(B): physically destroy per NIST SP 800-88 and "verify destruction in accordance with a defined process to demonstrate 100% effectiveness". Core 7(c)(3): annual validation audits.
- Appendix B (data sanitization), B(9): "mandatory video recordings of the process".
- No witness or second-person requirement was found.

## e-Stewards V4.1
- §8.9: NAID AAA certification is required as of 2022-07-01, so the destruction evidence it expects is NAID's.

## Synthesis used by ADR-0011
- **Minimum record per drive:** identifiers (serial plus asset tag), customer, technique, equipment, date/time, location, operator name/title/signature, verification of the remnants, a video reference, and the batch it belongs to.
- **No second person is required.** Concurrence is optional; video, screening and QC take the place of a two-person rule.
- **Recording before destruction is normal practice.** The honest record has two steps: "recorded for destruction", then "destruction completed".
- **A typed name plus signature is accepted everywhere.** Verified identity (SSO) strengthens how hard a record is to repudiate, in line with NAID's "verifiable electronic audit trail" standard.

## Statement wording (from the research; adopted in the spec)
- **Recorded for destruction:** "I, [name, title], recorded the storage device(s) listed above for destruction by [technique] using [equipment] at [location]. I attest that the identifiers shown were captured from the devices themselves and that these devices are being placed into the destruction process now. I attest that the information provided on this statement is accurate to the best of my knowledge."
- **Completion:** "Destruction of the device(s) listed above was completed on [date/time]. Verification: the remnants were inspected and the equipment [ID] completed its cycle; the result meets [criterion]. Video reference: [ID]." Say "Destroy" only for the five NIST techniques.
- **Concurrence:** "I, [name, title], [witnessed the destruction / reviewed the destruction records and verification evidence] for manifest [ID] and concur that the devices listed were destroyed as stated."
