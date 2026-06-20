#[cfg(feature = "js_runtime")]
use std::path::Path;
#[cfg(feature = "js_runtime")]
use oxc_allocator::Allocator;
#[cfg(feature = "js_runtime")]
use oxc_parser::Parser;
#[cfg(feature = "js_runtime")]
use oxc_span::SourceType;
#[cfg(feature = "js_runtime")]
use oxc_transformer::{Transformer, TransformOptions};
#[cfg(feature = "js_runtime")]
use oxc_codegen::{Codegen, CodegenOptions};

#[cfg(feature = "js_runtime")]
pub fn transpile_ts_to_js(source: &str, file_path: &Path) -> Result<String, String> {
    let allocator = Allocator::default();
    
    let source_type = SourceType::from_path(file_path)
        .unwrap_or_else(|_| SourceType::default().with_typescript(true));

    let ret = Parser::new(&allocator, source, source_type).parse();
    if !ret.errors.is_empty() {
        let errs = ret.errors.iter().map(|e| format!("{:?}", e)).collect::<Vec<_>>().join("\n");
        return Err(format!("TypeScript parsing failed:\n{}", errs));
    }
    
    let trivias = ret.trivias;
    let mut program = ret.program;

    let options = TransformOptions::default();

    let _ = Transformer::new(&allocator, file_path, source_type, source, &trivias, options)
        .build(&mut program);

    let result = Codegen::<false>::new(source, "", CodegenOptions::default(), None).build(&program);
    Ok(result.source_text)
}
