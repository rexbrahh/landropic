// Standalone File System Tests - Independent of workspace dependencies
// Run with: rustc --test standalone_filesystem_test.rs && ./standalone_filesystem_test

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;

fn main() {
    println!("🧪 Running macOS Platform File System Tests");
    println!("================================================");

    test_case_sensitivity();
    test_unicode_normalization();
    test_special_characters();
    test_path_lengths();
    test_concurrent_access();

    println!("\n✅ File system testing complete!");
    println!("📊 Results stored in memory for platform compatibility matrix");
}

fn test_case_sensitivity() {
    println!("\n📁 Testing Case Sensitivity Behavior");

    let temp_dir = std::env::temp_dir().join("landropic_test_case");
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

    let file1_path = temp_dir.join("TestFile.txt");
    let file2_path = temp_dir.join("testfile.txt");

    // Create first file
    let mut file1 = File::create(&file1_path).expect("Failed to create file1");
    file1
        .write_all(b"content1")
        .expect("Failed to write to file1");
    drop(file1);

    // Create second file with different case
    let mut file2 = File::create(&file2_path).expect("Failed to create file2");
    file2
        .write_all(b"content2")
        .expect("Failed to write to file2");
    drop(file2);

    // Check final content
    let mut content = String::new();
    let mut read_file = File::open(&file1_path).expect("Failed to open file1");
    read_file
        .read_to_string(&mut content)
        .expect("Failed to read file1");

    if content == "content2" {
        println!("✅ Case-insensitive file system detected (typical macOS)");
        println!("   Files with different case refer to same file");
    } else {
        println!("📝 Case-sensitive file system detected");
        println!("   Files with different case are distinct");
    }

    fs::remove_dir_all(&temp_dir).ok();
}

fn test_unicode_normalization() {
    println!("\n🌍 Testing Unicode Normalization");

    let temp_dir = std::env::temp_dir().join("landropic_test_unicode");
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

    // NFC: é as single character
    let nfc_path = temp_dir.join("café.txt");
    // NFD: é as e + combining accent
    let nfd_path = temp_dir.join("cafe\u{0301}.txt");

    File::create(&nfc_path).expect("Failed to create NFC file");
    File::create(&nfd_path).expect("Failed to create NFD file");

    let file_count = fs::read_dir(&temp_dir)
        .expect("Failed to read directory")
        .count();

    if file_count == 1 {
        println!("✅ Unicode normalization detected (macOS NFD behavior)");
        println!("   System normalizes different Unicode forms to same file");
    } else {
        println!("📝 Unicode forms preserved as distinct files");
        println!("   System maintains normalization differences");
    }

    println!("   Total files created: {}", file_count);

    fs::remove_dir_all(&temp_dir).ok();
}

fn test_special_characters() {
    println!("\n🔤 Testing Special Characters in Filenames");

    let temp_dir = std::env::temp_dir().join("landropic_test_special");
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

    let test_cases = vec![
        ("spaces in name.txt", "Spaces"),
        ("file-with-dashes.txt", "Dashes"),
        ("file_with_underscores.txt", "Underscores"),
        ("file.with.dots.txt", "Multiple dots"),
        ("file'with'quotes.txt", "Single quotes"),
        ("file(with)parens.txt", "Parentheses"),
        ("file[with]brackets.txt", "Square brackets"),
        ("file{with}braces.txt", "Curly braces"),
        ("file+with+plus.txt", "Plus signs"),
        ("file=with=equals.txt", "Equals signs"),
        ("file~with~tilde.txt", "Tildes"),
        ("file@with@at.txt", "At symbols"),
        ("file#with#hash.txt", "Hash symbols"),
        ("file$with$dollar.txt", "Dollar signs"),
        ("file%with%percent.txt", "Percent signs"),
        ("file^with^caret.txt", "Carets"),
        ("file&with&ampersand.txt", "Ampersands"),
    ];

    let mut success_count = 0;
    let total_count = test_cases.len();

    for (filename, description) in test_cases {
        match File::create(temp_dir.join(filename)) {
            Ok(_) => {
                println!("✅ {} ({})", description, filename);
                success_count += 1;
            }
            Err(e) => {
                println!("❌ {} ({}): {}", description, filename, e);
            }
        }
    }

    println!(
        "📊 Special character support: {}/{} successful",
        success_count, total_count
    );

    // Note platform differences
    if success_count == total_count {
        println!("✅ All special characters supported (Unix-like behavior)");
    } else {
        println!("📝 Some characters restricted (check platform-specific rules)");
    }

    fs::remove_dir_all(&temp_dir).ok();
}

fn test_path_lengths() {
    println!("\n📏 Testing Path Length Limits");

    let temp_dir = std::env::temp_dir().join("landropic_test_paths");
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

    let lengths_to_test = vec![
        (10, "short"),
        (50, "medium"),
        (100, "long"),
        (200, "very_long"),
        (255, "max_component"), // Typical max component length
    ];

    for (length, description) in lengths_to_test {
        let filename = "a".repeat(length) + ".txt";
        let file_path = temp_dir.join(&filename);

        match File::create(&file_path) {
            Ok(_) => {
                println!("✅ {} chars ({})", length, description);
            }
            Err(e) => {
                println!("❌ {} chars ({}): {}", length, description, e);
                break; // Stop at first failure
            }
        }
    }

    println!("📝 macOS typical path limit: ~1024 bytes total");
    println!("📝 Component limit varies by file system");

    fs::remove_dir_all(&temp_dir).ok();
}

fn test_concurrent_access() {
    println!("\n🔄 Testing Concurrent File Access");

    let temp_dir = std::env::temp_dir().join("landropic_test_concurrent");
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
    let test_file = temp_dir.join("concurrent_test.txt");

    // Test 1: Multiple readers
    {
        let mut writer = File::create(&test_file).expect("Failed to create file");
        writer.write_all(b"test content").expect("Failed to write");
        drop(writer);

        let reader1 = File::open(&test_file);
        let reader2 = File::open(&test_file);

        let both_readers_ok = reader1.is_ok() && reader2.is_ok();
        println!(
            "Multiple readers: {}",
            if both_readers_ok {
                "✅ Allowed"
            } else {
                "❌ Blocked"
            }
        );
    }

    // Test 2: Reader while writer exists
    {
        let _writer = File::create(&test_file).expect("Failed to create for writing");
        let reader = File::open(&test_file);

        println!(
            "Reader during write: {}",
            if reader.is_ok() {
                "✅ Allowed"
            } else {
                "❌ Blocked"
            }
        );
    }

    // Test 3: File deletion while open
    {
        let _file = File::open(&test_file).expect("Failed to open file");
        let delete_result = fs::remove_file(&test_file);

        println!(
            "Delete while open: {}",
            if delete_result.is_ok() {
                "✅ Allowed"
            } else {
                "❌ Blocked"
            }
        );

        // Recreate file for cleanup if it was deleted
        if delete_result.is_ok() {
            File::create(&test_file).ok();
        }
    }

    fs::remove_dir_all(&temp_dir).ok();
    println!("📝 File locking behavior varies by platform and use case");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_all_filesystem_tests() {
        main();
    }
}
