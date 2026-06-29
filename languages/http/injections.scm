; Highlight request/response bodies with the matching language. The grammar
; classifies a body by its leading characters (e.g. `{`/`[` + whitespace =>
; json_body, `<...` => xml_body), so highlighting follows the body's shape
; rather than the Content-Type header.

((json_body) @injection.content
  (#set! injection.language "json"))

((xml_body) @injection.content
  (#set! injection.language "xml"))

((graphql_data) @injection.content
  (#set! injection.language "graphql"))
