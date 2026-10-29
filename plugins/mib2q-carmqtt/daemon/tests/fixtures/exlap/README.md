Captured live from the unit on 2026-09-25 (FeCs 00060100 + 00060A00 present),
credential RSE_3-DE1400 unless the URL needed another. VIN replaced with
TESTVIN0000000000.

- `get_responses.json`: URL -> the server's full `<Rsp>` to `<Get url=.../>`
  (whitespace collapsed). Subscriptions push the same body as
  `<Dat timeStamp="..." url="...">...</Dat>`.
- `directory.json`: credential -> URL list from `<Dir urlPattern="*"/>`.

Handshake shapes seen on the wire:
    <Status>\n  <Init/>\n</Status>
    <Rsp id="1">\n  <Capabilities description="..." service="VW SAI-Server HIGH" version="5.0" id="..."><Supports protocol="1.3" interface="true" authenticate="true" heartbeat="true" datTimeStamp="true"/></Capabilities>\n</Rsp>
    <Rsp id="2"><Challenge nonce="tDJMI/KP3mVbdlD97P2p5A=="/></Rsp>
    <Rsp id="3"/>                                   (auth ok)
    <Rsp id="3" status="authenticationFailed"/>     (auth rejected)
    <Rsp id="4" status="noMatchingUrl"/>            (URL not visible to this login)
