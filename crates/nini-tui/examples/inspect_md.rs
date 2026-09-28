use nini_tui::markdown::render_markdown;
use nini_tui::theme::Theme;

fn main() {
    let theme = Theme::dark();
    let md = "- item 1\n- item 2\n";
    for (i, line) in render_markdown(md, &theme).iter().enumerate() {
        let s: String = line.spans.iter().map(|sp| sp.content.as_ref()).collect();
        println!("{:2}: |{}|", i, s);
    }
}