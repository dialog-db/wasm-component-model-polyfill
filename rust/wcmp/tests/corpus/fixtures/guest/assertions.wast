(assert_return (invoke "double" (u32.const 21)) (u32.const 42))
(assert_return (invoke "double" (u32.const 0)) (u32.const 0))
