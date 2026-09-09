def "http get" [url: string --headers: record] { {url: $url, headers: $headers} }
def "http patch" [url: string] { {url: $url} }
def "http delete" [url: string --data: any --content-type: string] { {url: $url, body: $data, content_type: $content_type} }
source client.nu
source models.nu
let result = (list-workspaces --base-url https://example.test --status [active paused] --namespace [a b] --tofu-version [1 2] --action [true false] --wid [w1 w2] --x-flags [true false])
let query = ($result.url | url parse | get params)
for pair in [[status ['active,paused']] [namespace [a b]] [tofu_version ['1,2']] [action ['true,false']] [wid [w1 w2]]] {
    let got = ($query | where key == $pair.0 | get value)
    if $got != $pair.1 { error make {msg: $"Wrong ($pair.0): ($got)"} }
}
if $result.headers.X-Flags != 'true,false' { error make {msg: 'Wrong header encoding'} }
let empty = (list-workspaces --base-url https://example.test --status [] --namespace [] --tofu-version [] --action [] --wid [] --x-flags [])
if $empty.url != 'https://example.test/v1/workspaces' { error make {msg: 'empty array was sent'} }
let update = (update-actions-for-workspace job workspace --base-url https://example.test)
if $update.url != 'https://example.test/v1/workspaces/workspace/actions/job' { error make {msg: 'path interpolation failed'} }
let deleted = (delete-workspace-variables workspace --base-url https://example.test --body [a b])
if $deleted.body != [a b] or $deleted.content_type != 'application/json' { error make {msg: 'DELETE body failed'} }
let flag_model = (make-env --hidden false)
if $flag_model.hidden != false { error make {msg: 'explicit false flag failed'} }
