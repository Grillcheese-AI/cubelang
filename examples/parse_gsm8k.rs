//! Parse the gsm8k.cube example and print the AST summary.

fn main() {
    let source = include_str!("gsm8k.cube");
    match cubelang::parser::parse(source) {
        Ok(ast) => {
            println!("Parsed {} top-level items:\n", ast.items.len());
            for item in &ast.items {
                match item {
                    cubelang::ast::TopLevel::Interface(i) => {
                        println!("  interface {} ({} members)", i.name, i.members.len());
                    }
                    cubelang::ast::TopLevel::Struct(s) => {
                        println!("  struct {} ({} fields)", s.name, s.fields.len());
                    }
                    cubelang::ast::TopLevel::Enum(e) => {
                        println!("  enum {} ({} variants)", e.name, e.variants.len());
                    }
                    cubelang::ast::TopLevel::Program(p) => {
                        println!("  program {} implements {:?} ({} items)", p.name, p.implements, p.body.len());
                        for item in &p.body {
                            match item {
                                cubelang::ast::ProgramItem::TypeAlias(t) => println!("    type {} = ...", t.name),
                                cubelang::ast::ProgramItem::Storage(s) => println!("    storage ({} fields, global={})", s.fields.len(), s.is_global),
                                cubelang::ast::ProgramItem::Constructor(f) => println!("    constructor ({} stmts)", f.body.len()),
                                cubelang::ast::ProgramItem::Function(f) => {
                                    let perms: Vec<_> = f.sig.permissions.iter().map(|p| format!("{:?}", p)).collect();
                                    let mods: Vec<_> = f.sig.modifiers.iter().map(|m| format!("{:?}", m)).collect();
                                    println!("    fn {}({}) -> {:?} [{}] [{}] ({} stmts)",
                                        f.sig.name,
                                        f.sig.params.iter().map(|p| format!("{}: {:?}", p.name, p.ty)).collect::<Vec<_>>().join(", "),
                                        f.sig.return_type,
                                        perms.join(", "),
                                        mods.join(", "),
                                        f.body.len());
                                }
                            }
                        }
                    }
                    cubelang::ast::TopLevel::Container(c) => {
                        println!("  {:?} {} extends {:?} implements {:?}", c.kind, c.name, c.extends, c.implements);
                    }
                    _ => println!("  other: {:?}", std::mem::discriminant(item)),
                }
            }
        }
        Err(e) => {
            eprintln!("Parse error: {}", e);
            std::process::exit(1);
        }
    }
}
