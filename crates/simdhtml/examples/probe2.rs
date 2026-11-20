use simdhtml::markup::scan;

fn main() {
    let input = "<!DOCTYPE html><pre>&#x0a;&#x0a;A</pre>";
    let index = scan(input.as_bytes()).unwrap();

    println!("tag_close={:?}", index.tag_close);
    println!(
        "text_runs={:?}",
        index
            .text_runs
            .iter()
            .map(|r| (r.start, r.end, &input[r.start as usize..r.end as usize]))
            .collect::<Vec<_>>()
    );

    for r in &index.opaque_regions {
        println!("region {:?} {:?}", r.kind, r.span);
    }
}
